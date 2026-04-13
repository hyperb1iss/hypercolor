//! Effect engine — manages the active effect and delegates to the renderer.
//!
//! The [`EffectEngine`] is the single orchestrator that owns the current
//! renderer, manages lifecycle transitions, and produces frames on demand.

use std::collections::HashMap;
use std::sync::LazyLock;

use tracing::{debug, error, info, warn};

use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{Canvas, DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH};
use hypercolor_types::effect::{
    ControlBinding, ControlDefinition, ControlKind, ControlValidationError, ControlValue,
    EffectMetadata, EffectState,
};
use hypercolor_types::sensor::SystemSnapshot;

use super::factory::create_renderer_for_metadata;
use super::traits::{EffectRenderer, FrameInput, prepare_target_canvas};
use crate::input::{InteractionData, ScreenData};

static EMPTY_SYSTEM_SNAPSHOT: LazyLock<SystemSnapshot> = LazyLock::new(SystemSnapshot::empty);

#[derive(Debug, Clone, PartialEq)]
struct ActiveBindingState {
    sensor_value: Option<f32>,
    control_value: ControlValue,
}

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

    /// Runtime state for controls currently managed by live sensor bindings.
    binding_state: HashMap<String, ActiveBindingState>,

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

    /// Monotonically increasing scene generation for frame-boundary snapshots.
    scene_generation: u64,
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
            binding_state: HashMap::new(),
            elapsed_secs: 0.0,
            frame_number: 0,
            canvas_width: DEFAULT_CANVAS_WIDTH,
            canvas_height: DEFAULT_CANVAS_HEIGHT,
            active_preset_id: None,
            scene_generation: 0,
        }
    }

    /// Create an engine with a custom canvas resolution.
    #[must_use]
    pub fn with_canvas_size(mut self, width: u32, height: u32) -> Self {
        self.canvas_width = width;
        self.canvas_height = height;
        self
    }

    /// Update the canvas resolution for subsequent frames.
    ///
    /// The active renderer (if any) continues running — it picks up the new
    /// dimensions from `FrameInput` on the next tick. No teardown/re-init
    /// needed because renderers read canvas size from the input, not from
    /// their initialization parameters.
    pub fn set_canvas_size(&mut self, width: u32, height: u32) {
        if self.canvas_width == width && self.canvas_height == height {
            return;
        }
        info!(
            old_width = self.canvas_width,
            old_height = self.canvas_height,
            new_width = width,
            new_height = height,
            "Canvas resize"
        );
        self.canvas_width = width;
        self.canvas_height = height;
        self.touch_scene();
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

    /// Returns the current scene generation.
    #[must_use]
    pub fn scene_generation(&self) -> u64 {
        self.scene_generation
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

        match renderer.init_with_canvas_size(&metadata, self.canvas_width, self.canvas_height) {
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
                self.binding_state.clear();
                for (name, value) in &self.controls {
                    renderer.set_control(name, value);
                }
                self.renderer = Some(renderer);
                self.metadata = Some(metadata);
                self.elapsed_secs = 0.0;
                self.frame_number = 0;
                self.active_preset_id = None;
                self.state = EffectState::Running;
                self.touch_scene();
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
        let had_active_effect = self.renderer.is_some()
            || self.metadata.is_some()
            || !self.controls.is_empty()
            || self.active_preset_id.is_some()
            || self.state != EffectState::Loading;
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
        self.binding_state.clear();
        self.elapsed_secs = 0.0;
        self.frame_number = 0;
        self.active_preset_id = None;
        self.state = EffectState::Loading;
        if had_active_effect {
            self.touch_scene();
        }
    }

    /// Pause the active effect. Renderer stays alive but `tick` returns
    /// an empty canvas.
    pub fn pause(&mut self) {
        if self.state == EffectState::Running {
            debug!("Effect paused");
            self.state = EffectState::Paused;
            self.touch_scene();
        } else {
            warn!(state = ?self.state, "Cannot pause — effect is not running");
        }
    }

    /// Resume a paused effect.
    pub fn resume(&mut self) {
        if self.state == EffectState::Paused {
            debug!("Effect resumed");
            self.state = EffectState::Running;
            self.touch_scene();
        } else {
            warn!(state = ?self.state, "Cannot resume — effect is not paused");
        }
    }

    /// Update a control parameter value.
    ///
    /// The value is stored and forwarded to the active renderer. If no
    /// renderer is active, the value is stored for when one is activated.
    pub fn set_control(&mut self, name: &str, value: &ControlValue) {
        let target_name = self
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.control_by_id(name))
            .map_or_else(
                || name.to_owned(),
                |control| control.control_id().to_owned(),
            );
        let changed = self.controls.get(&target_name) != Some(value);
        self.controls.insert(target_name.clone(), value.clone());
        if !self.control_has_binding(&target_name)
            && let Some(ref mut renderer) = self.renderer
        {
            renderer.set_control(&target_name, value);
        }
        self.active_preset_id = None;
        if changed {
            self.touch_scene();
        }
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

    #[must_use]
    pub fn preview_canvas(&self) -> Option<Canvas> {
        self.renderer.as_ref().and_then(|renderer| renderer.preview_canvas())
    }

    /// Attach or replace a live sensor binding for an active control.
    ///
    /// # Errors
    ///
    /// Returns an error if no effect is active, the control is unknown, or the
    /// binding uses an invalid sensor/range configuration.
    pub fn set_control_binding(
        &mut self,
        name: &str,
        binding: ControlBinding,
    ) -> anyhow::Result<ControlBinding> {
        let normalized = binding.normalized();
        let Some(metadata) = self.metadata.as_mut() else {
            return Err(anyhow::anyhow!("No active effect"));
        };
        let Some(control) = metadata.control_by_id_mut(name) else {
            return Err(anyhow::anyhow!("Unknown control '{name}'"));
        };

        validate_control_binding(control, &normalized)?;

        let control_id = control.control_id().to_owned();
        control.binding = Some(normalized.clone());
        self.binding_state.remove(&control_id);
        self.active_preset_id = None;
        self.touch_scene();
        Ok(normalized)
    }

    /// Set the active preset ID (called after preset controls are successfully applied).
    pub fn set_active_preset_id(&mut self, id: String) {
        if self.active_preset_id.as_deref() != Some(id.as_str()) {
            self.active_preset_id = Some(id);
            self.touch_scene();
        }
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

        let mut changed = self.active_preset_id.is_some();

        for control in &metadata.controls {
            self.binding_state.remove(control.control_id());
            let previous = self.controls.insert(
                control.control_id().to_owned(),
                control.default_value.clone(),
            );
            changed |= previous.as_ref() != Some(&control.default_value);
            if let Some(ref mut renderer) = self.renderer {
                renderer.set_control(control.control_id(), &control.default_value);
            }
        }

        self.active_preset_id = None;
        if changed {
            self.touch_scene();
        }
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
        let mut canvas = Canvas::new(self.canvas_width, self.canvas_height);
        self.tick_with_inputs_and_sensors_into(
            delta_secs,
            audio,
            &InteractionData::default(),
            None,
            &EMPTY_SYSTEM_SNAPSHOT,
            &mut canvas,
        )?;
        Ok(canvas)
    }

    /// Produce a single frame into caller-owned target storage.
    ///
    /// # Errors
    ///
    /// Returns an error if the renderer's render call fails.
    pub fn tick_into(
        &mut self,
        delta_secs: f32,
        audio: &AudioData,
        target: &mut Canvas,
    ) -> anyhow::Result<()> {
        self.tick_with_inputs_and_sensors_into(
            delta_secs,
            audio,
            &InteractionData::default(),
            None,
            &EMPTY_SYSTEM_SNAPSHOT,
            target,
        )
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
        let mut canvas = Canvas::new(self.canvas_width, self.canvas_height);
        self.tick_with_inputs_and_sensors_into(
            delta_secs,
            audio,
            interaction,
            None,
            &EMPTY_SYSTEM_SNAPSHOT,
            &mut canvas,
        )?;
        Ok(canvas)
    }

    /// Produce a single frame with host interaction state into caller-owned storage.
    ///
    /// # Errors
    ///
    /// Returns an error if the renderer's render call fails.
    pub fn tick_with_interaction_into(
        &mut self,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        target: &mut Canvas,
    ) -> anyhow::Result<()> {
        self.tick_with_inputs_and_sensors_into(
            delta_secs,
            audio,
            interaction,
            None,
            &EMPTY_SYSTEM_SNAPSHOT,
            target,
        )
    }

    /// Produce a single frame with host interaction state and optional screen input.
    ///
    /// Native screen-reactive effects use `screen` to render from the latest
    /// capture snapshot while HTML effects can safely ignore it until a binary
    /// screen transport exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the renderer's `tick` call fails.
    pub fn tick_with_inputs(
        &mut self,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
    ) -> anyhow::Result<Canvas> {
        let mut canvas = Canvas::new(self.canvas_width, self.canvas_height);
        self.tick_with_inputs_and_sensors_into(
            delta_secs,
            audio,
            interaction,
            screen,
            &EMPTY_SYSTEM_SNAPSHOT,
            &mut canvas,
        )?;
        Ok(canvas)
    }

    /// Produce a single frame with optional screen input into caller-owned storage.
    ///
    /// # Errors
    ///
    /// Returns an error if the renderer's render call fails.
    pub fn tick_with_inputs_into(
        &mut self,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        target: &mut Canvas,
    ) -> anyhow::Result<()> {
        self.tick_with_inputs_and_sensors_into(
            delta_secs,
            audio,
            interaction,
            screen,
            &EMPTY_SYSTEM_SNAPSHOT,
            target,
        )
    }

    /// Produce a single frame with optional screen input and a live sensor snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error if the renderer's render call fails.
    pub fn tick_with_inputs_and_sensors_into(
        &mut self,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
        target: &mut Canvas,
    ) -> anyhow::Result<()> {
        // If not running, return a blank canvas
        if self.state != EffectState::Running {
            prepare_target_canvas(target, self.canvas_width, self.canvas_height);
            target.clear();
            return Ok(());
        }

        let Some(ref mut renderer) = self.renderer else {
            prepare_target_canvas(target, self.canvas_width, self.canvas_height);
            target.clear();
            return Ok(());
        };

        self.elapsed_secs += delta_secs;
        apply_sensor_bindings(
            renderer.as_mut(),
            self.metadata.as_ref(),
            &self.controls,
            &mut self.binding_state,
            sensors,
        );

        let input = FrameInput {
            time_secs: self.elapsed_secs,
            delta_secs,
            frame_number: self.frame_number,
            audio,
            interaction,
            screen,
            sensors,
            canvas_width: self.canvas_width,
            canvas_height: self.canvas_height,
        };

        renderer.render_into(&input, target)?;
        self.frame_number += 1;

        Ok(())
    }

    fn touch_scene(&mut self) {
        self.scene_generation = self.scene_generation.wrapping_add(1);
    }

    fn control_has_binding(&self, name: &str) -> bool {
        self.metadata
            .as_ref()
            .and_then(|metadata| metadata.control_by_id(name))
            .and_then(|control| control.binding.as_ref())
            .is_some()
    }
}

impl Default for EffectEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn apply_sensor_bindings(
    renderer: &mut dyn EffectRenderer,
    metadata: Option<&EffectMetadata>,
    controls: &HashMap<String, ControlValue>,
    binding_state: &mut HashMap<String, ActiveBindingState>,
    sensors: &SystemSnapshot,
) {
    let Some(metadata) = metadata else {
        binding_state.clear();
        return;
    };

    for control in &metadata.controls {
        let control_id = control.control_id();
        let Some(binding) = control.binding.as_ref() else {
            if binding_state.remove(control_id).is_some()
                && let Some(base_value) = controls.get(control_id)
            {
                renderer.set_control(control_id, base_value);
            }
            continue;
        };

        let Some(base_value) = controls.get(control_id) else {
            continue;
        };

        let next_state = sensors
            .reading(&binding.sensor)
            .and_then(|reading| {
                evaluate_sensor_binding(
                    control,
                    binding,
                    reading.value,
                    binding_state.get(control_id),
                )
                .map(|value| ActiveBindingState {
                    sensor_value: Some(reading.value),
                    control_value: value,
                })
            })
            .unwrap_or_else(|| ActiveBindingState {
                sensor_value: None,
                control_value: base_value.clone(),
            });

        if binding_state.get(control_id) != Some(&next_state) {
            renderer.set_control(control_id, &next_state.control_value);
        }
        binding_state.insert(control_id.to_owned(), next_state);
    }
}

fn evaluate_sensor_binding(
    control: &ControlDefinition,
    binding: &ControlBinding,
    sensor_value: f32,
    previous: Option<&ActiveBindingState>,
) -> Option<ControlValue> {
    let source_span = binding.sensor_max - binding.sensor_min;
    if !source_span.is_finite()
        || source_span.abs() < f32::EPSILON
        || !binding.target_min.is_finite()
        || !binding.target_max.is_finite()
    {
        return None;
    }

    if let Some(previous) = previous
        && let Some(previous_sensor) = previous.sensor_value
        && (sensor_value - previous_sensor).abs() <= binding.deadband
    {
        return Some(previous.control_value.clone());
    }

    let normalized = ((sensor_value - binding.sensor_min) / source_span).clamp(0.0, 1.0);
    let mapped = binding.target_min + normalized * (binding.target_max - binding.target_min);
    let smoothed = previous
        .and_then(|state| state.control_value.as_f32())
        .map_or(mapped, |previous_value| {
            let alpha = 1.0 - binding.smoothing;
            previous_value + (mapped - previous_value) * alpha
        });

    match control.kind {
        ControlKind::Number | ControlKind::Hue | ControlKind::Area => {
            control.validate_value(&ControlValue::Float(smoothed)).ok()
        }
        ControlKind::Boolean => {
            let midpoint = binding.target_min + (binding.target_max - binding.target_min) * 0.5;
            control
                .validate_value(&ControlValue::Boolean(smoothed >= midpoint))
                .ok()
        }
        _ => None,
    }
}

fn validate_control_binding(
    control: &ControlDefinition,
    binding: &ControlBinding,
) -> anyhow::Result<()> {
    if binding.sensor.is_empty() {
        return Err(anyhow::anyhow!(
            "Control '{}' requires a non-empty sensor label",
            control.control_id()
        ));
    }

    if !matches!(
        control.kind,
        ControlKind::Number | ControlKind::Boolean | ControlKind::Hue | ControlKind::Area
    ) {
        return Err(anyhow::anyhow!(
            "Control '{}' does not support sensor bindings",
            control.control_id()
        ));
    }

    if !binding.sensor_min.is_finite()
        || !binding.sensor_max.is_finite()
        || !binding.target_min.is_finite()
        || !binding.target_max.is_finite()
    {
        return Err(anyhow::anyhow!(
            "Control '{}' binding range values must be finite",
            control.control_id()
        ));
    }

    if (binding.sensor_max - binding.sensor_min).abs() < f32::EPSILON {
        return Err(anyhow::anyhow!(
            "Control '{}' binding sensor range must not be zero",
            control.control_id()
        ));
    }

    Ok(())
}
