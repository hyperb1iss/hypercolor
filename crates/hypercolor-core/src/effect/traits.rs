//! Renderer trait and per-frame input data.
//!
//! [`EffectRenderer`] is the shared interface that both the wgpu and Servo
//! rendering backends implement. [`FrameInput`] carries all per-frame data
//! needed to produce a single canvas frame.

use std::sync::Arc;

pub use hypercolor_gpu_frame::{ImportedEffectFrame, ImportedFrameFormat, ImportedFrameTimings};
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::control::{ControlDeltaBatch, ControlSet};
use hypercolor_types::display::DisplayDescriptor;
use hypercolor_types::effect::EffectMetadata;
use hypercolor_types::lighting::LightingState;
use hypercolor_types::media::MediaState;
use hypercolor_types::net::NetStats;
use hypercolor_types::sensor::SystemSnapshot;
use tokio::sync::RwLock;

use crate::asset::AssetLibrary;
use crate::input::{InteractionData, ScreenBranchPublication};

// ── FrameInput ───────────────────────────────────────────────────────────────

/// Aggregate lifecycle state for the interaction source routed to an effect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputSourceAvailability {
    /// At least one eligible interaction source is routed to the render graph.
    pub routed: bool,

    /// At least one routed source is operational, including degraded operation.
    pub healthy: bool,

    /// At least one healthy routed source is within its freshness contract.
    pub fresh: bool,

    /// At least one routed source is operating with reduced capability.
    pub degraded: bool,
}

/// Typed, cadenced data sources injected alongside audio and sensors.
///
/// Each source is `None` until its producer delivers a snapshot (or on
/// platforms without one). Renderers that gate injection per effect read
/// these through [`FrameInput::sources`].
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameDataSources<'a> {
    /// Lifecycle state of the routed interaction source, independent of activity.
    pub input_availability: InputSourceAvailability,

    /// Now-playing media snapshot from the MPRIS source.
    pub media: Option<&'a MediaState>,

    /// Network throughput snapshot, refreshed at 1 Hz.
    pub net: Option<&'a NetStats>,

    /// What the rig is showing: scene, effects, dominant colors.
    pub lighting: Option<&'a LightingState>,
}

/// Per-frame input data passed to the active renderer on every render.
///
/// Contains timing information, the current audio analysis snapshot,
/// and the target canvas dimensions. Control values are delivered
/// separately via [`EffectRenderer::apply_controls`].
#[derive(Debug, Clone, Copy)]
pub struct FrameInput<'a> {
    /// Elapsed time in seconds since the effect was activated.
    pub time_secs: f64,

    /// Time delta since the previous frame, in seconds.
    pub delta_secs: f32,

    /// Monotonically increasing frame counter (starts at 0).
    pub frame_number: u64,

    /// Current audio analysis snapshot. Use [`AudioData::silence`]
    /// when no audio source is available.
    pub audio: &'a AudioData,

    /// Host keyboard and mouse state for interactive HTML effects.
    pub interaction: &'a InteractionData,

    /// Latest exact screen publication leased for screen-reactive effects.
    ///
    /// The snapshot is shared by reference count, so renderers that queue
    /// frames retain it without copying pixels. CPU renderers read only CPU
    /// surface and zone payloads; GPU-resident publications carry no CPU
    /// pixels and read as absent screen content.
    pub screen: Option<&'a Arc<ScreenBranchPublication>>,

    /// Latest system telemetry snapshot shared across all renderers.
    pub sensors: &'a SystemSnapshot,

    /// Typed data sources (media, net, lighting) for display faces.
    pub sources: FrameDataSources<'a>,

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

/// Frame output produced by an effect renderer.
#[derive(Debug, Clone)]
pub enum EffectRenderOutput {
    /// CPU-backed canvas pixels.
    Cpu(Canvas),
    /// GPU-resident imported texture.
    #[cfg(feature = "servo-gpu-import")]
    Gpu(ImportedEffectFrame),
    /// Renderer has no completed output for this frame.
    Pending,
}

impl EffectRenderOutput {
    /// Borrows the CPU canvas when this output is CPU-backed.
    #[must_use]
    pub fn as_cpu_canvas(&self) -> Option<&Canvas> {
        match self {
            Self::Cpu(canvas) => Some(canvas),
            #[cfg(feature = "servo-gpu-import")]
            Self::Gpu(_) => None,
            Self::Pending => None,
        }
    }

    /// Returns the CPU canvas when this output is CPU-backed.
    #[must_use]
    pub fn into_cpu_canvas(self) -> Option<Canvas> {
        match self {
            Self::Cpu(canvas) => Some(canvas),
            #[cfg(feature = "servo-gpu-import")]
            Self::Gpu(_) => None,
            Self::Pending => None,
        }
    }
}

// ── EffectRenderer ───────────────────────────────────────────────────────────

/// Shared rendering interface for all effect backends.
///
/// Both `WgpuRenderer` (native shaders) and `ServoRenderer` (HTML/Canvas)
/// implement this trait. `EffectPool` stores `Box<dyn EffectRenderer>`
/// instances per active zone and delegates frame production through them.
///
/// # Lifecycle
///
/// 1. **`init`**: Called once when the effect is activated. The renderer
///    should compile shaders, load resources, and prepare for rendering.
/// 2. **`render_into`**: Called once per frame. Produces pixels in a caller-
///    owned [`Canvas`] using the given [`FrameInput`].
/// 3. **`initialize_controls`**: Called with the authoritative snapshot
///    before the first frame after renderer creation or rebuild.
/// 4. **`apply_controls`**: Called with ordered atomic deltas whenever
///    authored or resolved values change. May be called between ticks.
/// 5. **`destroy`**: Called when the effect is deactivated. The renderer
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

    /// Produce a frame that may stay GPU-resident when the renderer supports it.
    ///
    /// The default keeps existing renderers on the CPU canvas contract.
    fn render_output(&mut self, input: &FrameInput<'_>) -> anyhow::Result<EffectRenderOutput> {
        let mut canvas = Canvas::new(input.canvas_width, input.canvas_height);
        self.render_into(input, &mut canvas)?;
        Ok(EffectRenderOutput::Cpu(canvas))
    }

    /// Advance an output-capable renderer without requiring the caller to
    /// consume a frame immediately.
    fn advance_output(&mut self, input: &FrameInput<'_>) -> anyhow::Result<()> {
        let _ = input;
        Ok(())
    }

    /// Initialize derived renderer state from the authoritative snapshot.
    ///
    /// The default delivers the complete snapshot as resolution sequence
    /// zero. Implementations may override this when replacing derived state
    /// needs behavior distinct from applying an ordinary delta.
    ///
    /// # Errors
    ///
    /// Returns an error when the renderer cannot replace its derived control
    /// state from the authoritative snapshot.
    fn initialize_controls(&mut self, controls: &ControlSet) -> anyhow::Result<()> {
        let changes = controls
            .iter()
            .map(|(control_id, value)| (control_id.clone(), value.clone()))
            .collect::<Vec<_>>();
        self.apply_controls(&ControlDeltaBatch::new(
            controls.set_revision(),
            0,
            &changes,
        ))
    }

    /// Apply one ordered batch of resolved control changes atomically.
    ///
    /// Implementations update only derived renderer caches. The owning effect
    /// slot retains the authoritative [`ControlSet`], and values have already
    /// passed canonical and effect-definition validation.
    ///
    /// # Errors
    ///
    /// Returns an error when the batch cannot be applied atomically to the
    /// renderer's derived state.
    fn apply_controls(&mut self, batch: &ControlDeltaBatch<'_>) -> anyhow::Result<()>;

    /// Bind the content-addressed asset library.
    ///
    /// Renderers that resolve uploaded media by asset id (the media-player
    /// effect) use this handle to look assets up against the library. The
    /// default no-op covers every renderer without asset-backed controls.
    fn bind_asset_library(&mut self, _library: Arc<RwLock<AssetLibrary>>) {}

    /// Describe the physical display surface this renderer targets.
    ///
    /// Set before [`init_with_canvas_size`](Self::init_with_canvas_size) for
    /// display-face renderers so the page can adapt to device truth (shape,
    /// safe area, fps). The default no-op covers every renderer that does
    /// not drive a device display.
    fn set_display_descriptor(&mut self, _descriptor: Option<DisplayDescriptor>) {}

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
