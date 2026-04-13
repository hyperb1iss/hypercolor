//! Renderer trait and per-frame input data.
//!
//! [`EffectRenderer`] is the shared interface that both the wgpu and Servo
//! rendering backends implement. [`FrameInput`] carries all per-frame data
//! needed to produce a single canvas frame.

use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::effect::{ControlValue, EffectMetadata};
use hypercolor_types::sensor::SystemSnapshot;

use crate::input::{InteractionData, ScreenData};

// ── FrameInput ───────────────────────────────────────────────────────────────

/// Per-frame input data passed to the active renderer on every tick.
///
/// Contains timing information, the current audio analysis snapshot,
/// and the target canvas dimensions. Control values are delivered
/// separately via [`EffectRenderer::set_control`].
#[derive(Debug, Clone, Copy)]
pub struct FrameInput<'a> {
    /// Elapsed time in seconds since the effect was activated.
    pub time_secs: f32,

    /// Time delta since the previous frame, in seconds.
    pub delta_secs: f32,

    /// Monotonically increasing frame counter (starts at 0).
    pub frame_number: u64,

    /// Current audio analysis snapshot. Use [`AudioData::silence`]
    /// when no audio source is available.
    pub audio: &'a AudioData,

    /// Host keyboard and mouse state for interactive HTML effects.
    pub interaction: &'a InteractionData,

    /// Latest screen-capture snapshot for screen-reactive effects.
    pub screen: Option<&'a ScreenData>,

    /// Latest system telemetry snapshot shared across all renderers.
    pub sensors: &'a SystemSnapshot,

    /// Target canvas width in pixels.
    pub canvas_width: u32,

    /// Target canvas height in pixels.
    pub canvas_height: u32,
}

/// Ensure a renderer target canvas matches the requested frame dimensions.
pub fn prepare_target_canvas(target: &mut Canvas, width: u32, height: u32) {
    if target.width() != width || target.height() != height {
        *target = Canvas::new(width, height);
    }
}

// ── EffectRenderer ───────────────────────────────────────────────────────────

/// Shared rendering interface for all effect backends.
///
/// Both `WgpuRenderer` (native shaders) and `ServoRenderer` (HTML/Canvas)
/// implement this trait. The [`EffectEngine`](super::EffectEngine) holds a
/// `Box<dyn EffectRenderer>` and delegates frame production through it.
///
/// # Lifecycle
///
/// 1. **`init`** — Called once when the effect is activated. The renderer
///    should compile shaders, load resources, and prepare for rendering.
/// 2. **`render_into`** — Called once per frame. Produces pixels in a caller-
///    owned [`Canvas`] using the given [`FrameInput`].
/// 3. **`set_control`** — Called whenever a control value changes (user
///    interaction, preset load, API call). May be called between ticks.
/// 4. **`destroy`** — Called when the effect is deactivated. The renderer
///    should release GPU resources, close web views, etc.
pub trait EffectRenderer: Send {
    /// Initialize the renderer for the given effect.
    ///
    /// Called once when the effect transitions from `Loading` to `Initializing`.
    /// The renderer should use the metadata to configure itself (shader source,
    /// canvas dimensions, audio reactivity, etc.).
    ///
    /// # Errors
    ///
    /// Returns an error if initialization fails (shader compilation, resource
    /// allocation, missing source files, etc.).
    fn init(&mut self, metadata: &EffectMetadata) -> anyhow::Result<()>;

    /// Initialize the renderer for the given effect and target canvas size.
    ///
    /// Renderers that need the final presentation size before their first
    /// frame can override this. Backends that do not care can keep the default
    /// behavior and defer size handling to [`render_into`](Self::render_into).
    fn init_with_canvas_size(
        &mut self,
        metadata: &EffectMetadata,
        canvas_width: u32,
        canvas_height: u32,
    ) -> anyhow::Result<()> {
        let _ = (canvas_width, canvas_height);
        self.init(metadata)
    }

    /// Produce a single frame into caller-owned target storage.
    ///
    /// Called once per render loop iteration while the effect is `Running`.
    /// The target [`Canvas`] is consumed by the spatial sampler and UI preview.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame cannot be produced (GPU fault, render
    /// timeout, etc.). The engine may retry or transition to an error state.
    fn render_into(&mut self, input: &FrameInput<'_>, target: &mut Canvas) -> anyhow::Result<()>;

    /// Produce a single frame.
    ///
    /// Legacy convenience wrapper that allocates a fresh target canvas and
    /// delegates to [`render_into`](Self::render_into).
    ///
    /// # Errors
    ///
    /// Returns an error if the frame cannot be produced.
    fn tick(&mut self, input: &FrameInput<'_>) -> anyhow::Result<Canvas> {
        let mut canvas = Canvas::new(input.canvas_width, input.canvas_height);
        self.render_into(input, &mut canvas)?;
        Ok(canvas)
    }

    /// Update a control parameter value.
    ///
    /// Called when a user adjusts a control, a preset is loaded, or the API
    /// pushes a value. The renderer should store the value and apply it on
    /// the next [`render_into`](Self::render_into) call.
    fn set_control(&mut self, name: &str, value: &ControlValue);

    /// Optional auxiliary preview canvas for control-panel tooling.
    ///
    /// Most effects do not expose a secondary preview stream. Effects that
    /// render a higher-resolution source image (for example a cropped webpage)
    /// can return it here so the daemon can publish it on demand.
    fn preview_canvas(&self) -> Option<Canvas> {
        None
    }

    /// Tear down the renderer and release all resources.
    ///
    /// Called when the effect transitions to `Destroying`. After this call,
    /// the renderer will not receive any further method calls.
    fn destroy(&mut self);
}
