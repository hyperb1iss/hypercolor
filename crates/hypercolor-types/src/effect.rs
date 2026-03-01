//! Effect metadata, controls, and lifecycle types.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};
use uuid::Uuid;

// ── EffectId ──────────────────────────────────────────────────────────────────

/// Unique identifier for an effect, wrapping a UUID v7.
///
/// Generated at discovery time and used as the primary key across
/// the registry, event bus, API, and UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EffectId(pub Uuid);

impl EffectId {
    /// Create a new `EffectId` from a `Uuid`.
    #[must_use]
    pub fn new(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Return the inner `Uuid`.
    #[must_use]
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl fmt::Display for EffectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for EffectId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

// ── EffectCategory ────────────────────────────────────────────────────────────

/// Primary classification categories for the effect taxonomy.
///
/// An effect can belong to multiple categories. Used for discovery
/// and filtering in the effect browser UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, EnumString, Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum EffectCategory {
    /// Ambient lighting: calm, atmospheric visuals.
    Ambient,
    /// Reactive effects that respond to system or device events.
    Reactive,
    /// Audio-reactive effects driven by music or microphone input.
    Audio,
    /// Gaming-oriented effects (flash, health bars, cooldown indicators).
    Gaming,
    /// Productivity effects (notification pulses, focus timers, pomodoro).
    Productivity,
    /// Utility effects (solid color, temperature display, battery level).
    Utility,
    /// Interactive effects that respond to user input (mouse, keyboard).
    Interactive,
    /// Generative art: procedural patterns, fractals, noise fields.
    Generative,
}

impl Default for EffectCategory {
    fn default() -> Self {
        Self::Ambient
    }
}

// ── EffectSource ──────────────────────────────────────────────────────────────

/// Identifies the rendering path and source location for an effect.
///
/// Determines which renderer handles the effect (wgpu vs. Servo).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectSource {
    /// Native WGSL/GLSL shader rendered by `WgpuRenderer`.
    Native {
        /// Path to the shader file, relative to the effects root.
        path: PathBuf,
    },
    /// HTML/Canvas/WebGL effect rendered by `ServoRenderer`.
    Html {
        /// Path to the `.html` file, relative to the effects root.
        path: PathBuf,
    },
    /// GPU compute or fragment shader in raw SPIR-V or WGSL.
    Shader {
        /// Path to the shader source file.
        path: PathBuf,
    },
}

impl EffectSource {
    /// Returns the path to the primary source file.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Native { path } | Self::Html { path } | Self::Shader { path } => path,
        }
    }
}

// ── EffectState ───────────────────────────────────────────────────────────────

/// Lifecycle state of an effect in the registry.
///
/// Tracks the effect from initial discovery through rendering and teardown.
/// Only one effect (or composition) can be `Running` at a time per render loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectState {
    /// Source files discovered, metadata being parsed and validated.
    Loading,
    /// Renderer being initialized: shader compiling (wgpu) or HTML loading (Servo).
    Initializing,
    /// Actively rendering frames to the canvas.
    Running,
    /// Renderer alive but not producing frames (used during transitions/crossfades).
    Paused,
    /// Renderer being torn down, GPU/Servo resources being freed.
    Destroying,
}

impl Default for EffectState {
    fn default() -> Self {
        Self::Loading
    }
}

// ── GradientStop ──────────────────────────────────────────────────────────────

/// A single stop in a color gradient.
///
/// Position is normalized `0.0..=1.0` along the gradient axis.
/// Color is stored as linear RGBA (`[f32; 4]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    /// Position along the gradient axis, `0.0` = start, `1.0` = end.
    pub position: f32,
    /// Linear RGBA color at this stop.
    pub color: [f32; 4],
}

// ── ControlType ───────────────────────────────────────────────────────────────

/// Widget kind for a user-facing effect control.
///
/// Each variant maps to a specific UI component in the control panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlType {
    /// Numeric slider with optional step quantization.
    Slider,
    /// On/off toggle switch.
    Toggle,
    /// RGBA color picker dialog.
    ColorPicker,
    /// Multi-stop gradient editor.
    GradientEditor,
    /// Selection from a fixed set of labeled options.
    Dropdown,
    /// Free-form single-line text input.
    TextInput,
}

// ── ControlValue ──────────────────────────────────────────────────────────────

/// Runtime value of a control parameter.
///
/// The variant must be compatible with the corresponding [`ControlType`]:
///
/// | `ControlType`    | Valid `ControlValue`       |
/// |------------------|----------------------------|
/// | `Slider`         | `Float(f32)`               |
/// | `Toggle`         | `Boolean(bool)`            |
/// | `ColorPicker`    | `Color([f32; 4])`          |
/// | `GradientEditor` | `Gradient(Vec<GradientStop>)` |
/// | `Dropdown`       | `Enum(String)`             |
/// | `TextInput`      | `Text(String)`             |
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlValue {
    /// Floating-point numeric value. Used by `Slider` controls.
    Float(f32),
    /// Signed integer value.
    Integer(i32),
    /// Boolean on/off value. Used by `Toggle` controls.
    Boolean(bool),
    /// Linear RGBA color. Used by `ColorPicker` controls.
    Color([f32; 4]),
    /// Multi-stop gradient. Used by `GradientEditor` controls.
    Gradient(Vec<GradientStop>),
    /// Named enum variant. Used by `Dropdown` controls.
    Enum(String),
    /// Free-form text. Used by `TextInput` controls.
    Text(String),
}

impl ControlValue {
    /// Returns the value as an `f32`, if numeric.
    ///
    /// `Float` returns the inner value directly.
    /// `Integer` converts via widening cast.
    /// `Boolean` returns `1.0` for `true`, `0.0` for `false`.
    /// All other variants return `None`.
    #[must_use]
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Self::Float(v) => Some(*v),
            #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
            Self::Integer(v) => Some(*v as f32),
            Self::Boolean(v) => Some(if *v { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    /// Returns a JavaScript-compatible literal for injection into
    /// Servo's `window[name]` globals.
    #[must_use]
    pub fn to_js_literal(&self) -> String {
        match self {
            Self::Float(v) => v.to_string(),
            Self::Integer(v) => v.to_string(),
            Self::Boolean(v) => if *v { "true" } else { "false" }.to_string(),
            Self::Color([r, g, b, a]) => {
                format!("[{r}, {g}, {b}, {a}]")
            }
            Self::Gradient(stops) => {
                let entries: Vec<String> = stops
                    .iter()
                    .map(|s| {
                        format!(
                            "{{pos:{},color:[{},{},{},{}]}}",
                            s.position, s.color[0], s.color[1], s.color[2], s.color[3]
                        )
                    })
                    .collect();
                format!("[{}]", entries.join(","))
            }
            Self::Enum(v) | Self::Text(v) => {
                format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
            }
        }
    }
}

// ── ControlDefinition ─────────────────────────────────────────────────────────

/// A single user-facing parameter declared by an effect.
///
/// The UI auto-generates widgets from these definitions. The engine
/// injects current values into the active renderer every frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlDefinition {
    /// Human-readable label shown in the control panel.
    pub name: String,
    /// Widget kind and constraints.
    pub control_type: ControlType,
    /// Initial value. Must be compatible with `control_type`.
    pub default_value: ControlValue,
    /// Minimum numeric bound (applicable to `Slider` controls).
    #[serde(default)]
    pub min: Option<f32>,
    /// Maximum numeric bound (applicable to `Slider` controls).
    #[serde(default)]
    pub max: Option<f32>,
    /// Step increment for numeric controls. `None` means continuous.
    #[serde(default)]
    pub step: Option<f32>,
    /// Labels for `Dropdown` options.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Optional grouping for the control panel UI.
    #[serde(default)]
    pub group: Option<String>,
    /// Help text shown on hover/focus.
    #[serde(default)]
    pub tooltip: Option<String>,
}

// ── EffectMetadata ────────────────────────────────────────────────────────────

/// Universal effect descriptor.
///
/// Serialized as TOML for native effects and as JSON for the REST API
/// and WebSocket protocol. This is the canonical metadata attached to
/// every effect regardless of rendering path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectMetadata {
    /// Stable unique identifier.
    pub id: EffectId,
    /// Human-readable display name.
    pub name: String,
    /// Author or publisher name.
    pub author: String,
    /// Semantic version string (e.g. `"1.2.0"`).
    #[serde(default = "default_version")]
    pub version: String,
    /// Short description (max 200 chars). Shown in the effect browser.
    pub description: String,
    /// Primary classification categories.
    #[serde(default)]
    pub category: EffectCategory,
    /// Discovery and taxonomy tags. Free-form, lowercase, hyphenated.
    #[serde(default)]
    pub tags: Vec<String>,
    /// How this effect is rendered. Determines the renderer path.
    pub source: EffectSource,
    /// SPDX license identifier (e.g. `"MIT"`, `"Apache-2.0"`).
    #[serde(default)]
    pub license: Option<String>,
}

fn default_version() -> String {
    "0.1.0".to_owned()
}
