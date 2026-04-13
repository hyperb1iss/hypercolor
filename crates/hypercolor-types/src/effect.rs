//! Effect metadata, controls, and lifecycle types.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};
use uuid::Uuid;

use crate::canvas::srgb_to_linear;
use crate::viewport::ViewportRect;

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
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, EnumString, Display, Default,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum EffectCategory {
    /// Slow, set-and-forget (aurora, breathing, gradient).
    #[default]
    Ambient,
    /// Music-reactive (spectrum, beat pulse, waveform).
    Audio,
    /// Algorithmic/mathematical (voronoi, fractals, CA).
    Generative,
    /// Physics simulations (fire, meteors, bubbles).
    Particle,
    /// Environmental compositions (cyberpunk city, underwater).
    Scenic,
    /// Input-responsive (keystroke ripple, heatmap).
    Interactive,
    /// Playful/seasonal (corner hunt, snowfall, dragonfire).
    Fun,
    /// Functional (solid color, off, system monitor).
    Utility,
    /// Full-fidelity HTML display faces for LCD surfaces.
    Display,
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
        /// Path to the `.html` file on disk.
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

    /// Returns the source file stem when it is valid UTF-8.
    #[must_use]
    pub fn source_stem(&self) -> Option<&str> {
        self.path().file_stem()?.to_str()
    }
}

// ── EffectState ───────────────────────────────────────────────────────────────

/// Lifecycle state of an effect in the registry.
///
/// Tracks the effect from initial discovery through rendering and teardown.
/// Only one effect (or composition) can be `Running` at a time per render loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EffectState {
    /// Source files discovered, metadata being parsed and validated.
    #[default]
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
    /// Interactive rectangular viewport picker.
    Rect,
}

// ── ControlKind ───────────────────────────────────────────────────────────────

/// Semantic control kind declared by an effect source.
///
/// This keeps `LightScript` metadata semantics intact even when
/// multiple kinds map to the same UI widget type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ControlKind {
    /// Generic numeric value.
    #[default]
    Number,
    /// Boolean on/off value.
    Boolean,
    /// Hex or structured color value.
    Color,
    /// Named option from a fixed set.
    Combobox,
    /// Sensor selector value (for example CPU/GPU metrics).
    Sensor,
    /// Hue wheel numeric value.
    Hue,
    /// Area/region scalar value.
    Area,
    /// Free-form text value.
    Text,
    /// Normalized rectangular viewport value.
    Rect,
    /// Unknown/unmapped kind from source metadata.
    Other(String),
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
/// | `Rect`           | `Rect(ViewportRect)`       |
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
    /// Normalized rectangular viewport.
    Rect(ViewportRect),
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
            Self::Color([r, g, b, _a]) => {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    clippy::as_conversions
                )]
                let (ri, gi, bi) = (
                    (r * 255.0).round() as u8,
                    (g * 255.0).round() as u8,
                    (b * 255.0).round() as u8,
                );
                format!("\"#{ri:02x}{gi:02x}{bi:02x}\"")
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
            Self::Rect(rect) => {
                format!(
                    "{{x:{},y:{},width:{},height:{}}}",
                    rect.x, rect.y, rect.width, rect.height
                )
            }
        }
    }
}

/// Validation errors for a control update payload.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ControlValidationError {
    #[error("control '{control}': expected numeric value, got {got}")]
    ExpectedNumeric { control: String, got: &'static str },
    #[error("control '{control}': expected boolean value, got {got}")]
    ExpectedBoolean { control: String, got: &'static str },
    #[error("control '{control}': expected color/text value, got {got}")]
    ExpectedColorLike { control: String, got: &'static str },
    #[error("control '{control}': expected text value, got {got}")]
    ExpectedText { control: String, got: &'static str },
    #[error("control '{control}': expected rect value, got {got}")]
    ExpectedRect { control: String, got: &'static str },
    #[error("control '{control}': invalid option '{value}', valid options: {valid:?}")]
    InvalidOption {
        control: String,
        value: String,
        valid: Vec<String>,
    },
}

fn control_value_kind(value: &ControlValue) -> &'static str {
    match value {
        ControlValue::Float(_) => "float",
        ControlValue::Integer(_) => "integer",
        ControlValue::Boolean(_) => "boolean",
        ControlValue::Color(_) => "color",
        ControlValue::Gradient(_) => "gradient",
        ControlValue::Enum(_) => "enum",
        ControlValue::Text(_) => "text",
        ControlValue::Rect(_) => "rect",
    }
}

fn parse_hex_color(text: &str) -> Option<[f32; 4]> {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }

    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;

    Some([
        srgb_to_linear(f32::from(r) / 255.0),
        srgb_to_linear(f32::from(g) / 255.0),
        srgb_to_linear(f32::from(b) / 255.0),
        1.0,
    ])
}

// ── ControlBinding ────────────────────────────────────────────────────────────

/// Live mapping from a system sensor reading into a control value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlBinding {
    /// Stable sensor label to sample from the current system snapshot.
    pub sensor: String,
    /// Lower bound of the source sensor range.
    pub sensor_min: f32,
    /// Upper bound of the source sensor range.
    pub sensor_max: f32,
    /// Lower bound of the mapped control range.
    pub target_min: f32,
    /// Upper bound of the mapped control range.
    pub target_max: f32,
    /// Minimum source-value delta required before the binding updates.
    #[serde(default)]
    pub deadband: f32,
    /// Temporal smoothing factor. `0.0` is immediate, `0.99` is very slow.
    #[serde(default)]
    pub smoothing: f32,
}

impl ControlBinding {
    /// Clamp and trim user-provided binding values into runtime-safe ranges.
    #[must_use]
    pub fn normalized(&self) -> Self {
        Self {
            sensor: self.sensor.trim().to_owned(),
            sensor_min: self.sensor_min,
            sensor_max: self.sensor_max,
            target_min: self.target_min,
            target_max: self.target_max,
            deadband: if self.deadband.is_finite() {
                self.deadband.max(0.0)
            } else {
                0.0
            },
            smoothing: if self.smoothing.is_finite() {
                self.smoothing.clamp(0.0, 0.99)
            } else {
                0.0
            },
        }
    }
}

/// Live preview stream a control should bind to in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewSource {
    ScreenCapture,
    WebViewport,
    EffectCanvas,
}

// ── ControlDefinition ─────────────────────────────────────────────────────────

/// A single user-facing parameter declared by an effect.
///
/// The UI auto-generates widgets from these definitions. The engine
/// injects current values into the active renderer every frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlDefinition {
    /// Stable control identifier used in API payloads and renderer globals.
    #[serde(default)]
    pub id: String,
    /// Human-readable label shown in the control panel.
    pub name: String,
    /// Semantic kind from source metadata (`LightScript`).
    #[serde(default)]
    pub kind: ControlKind,
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
    /// Optional fixed aspect ratio (`width / height`) for rect controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect_lock: Option<f32>,
    /// Optional preview stream used by composite controls in the UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_source: Option<PreviewSource>,
    /// Optional live sensor mapping for this control.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<ControlBinding>,
}

impl ControlDefinition {
    /// Returns a stable control id.
    ///
    /// For backward compatibility with older metadata payloads, falls back
    /// to `name` when `id` is unset.
    #[must_use]
    pub fn control_id(&self) -> &str {
        if self.id.is_empty() {
            &self.name
        } else {
            &self.id
        }
    }

    /// Validate and normalize a control value against this definition.
    pub fn validate_value(
        &self,
        value: &ControlValue,
    ) -> Result<ControlValue, ControlValidationError> {
        let control = self.control_id().to_owned();
        match self.kind {
            ControlKind::Number | ControlKind::Hue | ControlKind::Area => {
                let Some(mut normalized) = value.as_f32() else {
                    return Err(ControlValidationError::ExpectedNumeric {
                        control,
                        got: control_value_kind(value),
                    });
                };
                if let Some(min) = self.min {
                    normalized = normalized.max(min);
                }
                if let Some(max) = self.max {
                    normalized = normalized.min(max);
                }
                if let Some(step) = self.step.filter(|step| *step > 0.0) {
                    normalized = (normalized / step).round() * step;
                }
                Ok(ControlValue::Float(normalized))
            }
            ControlKind::Boolean => match value {
                ControlValue::Boolean(flag) => Ok(ControlValue::Boolean(*flag)),
                _ => Err(ControlValidationError::ExpectedBoolean {
                    control,
                    got: control_value_kind(value),
                }),
            },
            ControlKind::Combobox => {
                let candidate = match value {
                    ControlValue::Enum(option) | ControlValue::Text(option) => option.clone(),
                    _ => {
                        return Err(ControlValidationError::ExpectedText {
                            control,
                            got: control_value_kind(value),
                        });
                    }
                };

                if self.labels.is_empty()
                    || self
                        .labels
                        .iter()
                        .any(|option| option.eq_ignore_ascii_case(&candidate))
                {
                    Ok(ControlValue::Enum(candidate))
                } else {
                    Err(ControlValidationError::InvalidOption {
                        control,
                        value: candidate,
                        valid: self.labels.clone(),
                    })
                }
            }
            ControlKind::Rect => match value {
                ControlValue::Rect(rect) => Ok(ControlValue::Rect(rect.clamp())),
                _ => Err(ControlValidationError::ExpectedRect {
                    control,
                    got: control_value_kind(value),
                }),
            },
            ControlKind::Color => match value {
                ControlValue::Color(color) => Ok(ControlValue::Color(*color)),
                ControlValue::Text(text) | ControlValue::Enum(text) => {
                    if matches!(self.control_type, ControlType::ColorPicker)
                        && let Some(color) = parse_hex_color(text)
                    {
                        return Ok(ControlValue::Color(color));
                    }
                    Ok(ControlValue::Text(text.clone()))
                }
                _ => Err(ControlValidationError::ExpectedColorLike {
                    control,
                    got: control_value_kind(value),
                }),
            },
            ControlKind::Sensor | ControlKind::Text | ControlKind::Other(_) => match value {
                ControlValue::Text(text) | ControlValue::Enum(text) => {
                    Ok(ControlValue::Text(text.clone()))
                }
                _ => Err(ControlValidationError::ExpectedText {
                    control,
                    got: control_value_kind(value),
                }),
            },
        }
    }
}

// ── PresetTemplate ────────────────────────────────────────────────────────

/// An effect-defined preset — a named snapshot of control values bundled
/// with the effect itself. Unlike user-created [`super::library::EffectPreset`]s,
/// these are authored by the effect developer and are read-only at runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetTemplate {
    /// Human-readable preset name (e.g. "Sunset Glow", "Deep Ocean").
    pub name: String,
    /// Optional short description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Control values that define this preset. Keys are control IDs.
    #[serde(default)]
    pub controls: HashMap<String, ControlValue>,
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
    /// User-facing controls declared by this effect.
    #[serde(default)]
    pub controls: Vec<ControlDefinition>,
    /// Effect-defined preset snapshots. Authored by the effect developer,
    /// read-only at runtime. Shown alongside user-created presets in the UI.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<PresetTemplate>,
    /// Indicates whether the effect expects audio payload injection.
    #[serde(default)]
    pub audio_reactive: bool,
    /// Indicates whether the effect expects screen capture payload injection.
    #[serde(default)]
    pub screen_reactive: bool,
    /// How this effect is rendered. Determines the renderer path.
    pub source: EffectSource,
    /// SPDX license identifier (e.g. `"MIT"`, `"Apache-2.0"`).
    #[serde(default)]
    pub license: Option<String>,
}

impl EffectMetadata {
    /// Look up a control definition by id (case-insensitive).
    #[must_use]
    pub fn control_by_id(&self, id: &str) -> Option<&ControlDefinition> {
        self.controls
            .iter()
            .find(|control| control.control_id().eq_ignore_ascii_case(id))
    }

    /// Look up a mutable control definition by id (case-insensitive).
    pub fn control_by_id_mut(&mut self, id: &str) -> Option<&mut ControlDefinition> {
        self.controls
            .iter_mut()
            .find(|control| control.control_id().eq_ignore_ascii_case(id))
    }

    /// Match either the display name or a stable source-stem alias.
    #[must_use]
    pub fn matches_lookup(&self, id_or_name: &str) -> bool {
        self.name.eq_ignore_ascii_case(id_or_name)
            || self
                .source
                .source_stem()
                .is_some_and(|stem| stem.eq_ignore_ascii_case(id_or_name))
    }
}

fn default_version() -> String {
    "0.1.0".to_owned()
}
