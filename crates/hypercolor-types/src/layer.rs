//! Authored scene layer-stack types.

use std::collections::HashMap;
use std::fmt;
use std::ops::RangeInclusive;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::asset::AssetId;
use crate::canvas::BlendMode;
use crate::effect::{ControlBinding, ControlValue, EffectId};
use crate::library::PresetId;
use crate::scene::DisplayFaceBlendMode;
use crate::spatial::NormalizedPosition;
use crate::viewport::{FitMode, ViewportRect};

/// Stable identifier for a layer within a render group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SceneLayerId(pub Uuid);

impl SceneLayerId {
    /// Create a fresh UUID v7 layer identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Create a layer identifier from an existing UUID.
    #[must_use]
    pub const fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Return the wrapped UUID.
    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for SceneLayerId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SceneLayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for SceneLayerId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl FromStr for SceneLayerId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

/// Authored layer inside a render group's bottom-to-top stack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneLayer {
    /// Stable identifier for this layer.
    pub id: SceneLayerId,

    /// Display name. Defaults to the source's intrinsic name in the UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Content source that feeds this layer.
    pub source: LayerSource,

    /// How this layer composes with the layer beneath it.
    #[serde(default)]
    pub blend: LayerBlendMode,

    /// Layer opacity.
    #[serde(default = "default_layer_opacity")]
    pub opacity: f32,

    /// Geometric placement of the source within the group's canvas.
    #[serde(default)]
    pub transform: LayerTransform,

    /// Color adjustments applied after the source produces a frame.
    #[serde(default)]
    pub adjust: LayerAdjust,

    /// Live scalar bindings for layer parameters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<LayerBinding>,

    /// Whether this layer is currently active.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl SceneLayer {
    /// Create the legacy single-effect layer for a render group.
    #[must_use]
    pub fn from_effect(
        id: SceneLayerId,
        effect_id: EffectId,
        controls: HashMap<String, ControlValue>,
        control_bindings: HashMap<String, ControlBinding>,
        preset_id: Option<PresetId>,
    ) -> Self {
        Self {
            id,
            name: None,
            source: LayerSource::Effect {
                effect_id,
                controls,
                control_bindings,
                preset_id,
            },
            blend: LayerBlendMode::Replace,
            opacity: default_layer_opacity(),
            transform: LayerTransform::default(),
            adjust: LayerAdjust::default(),
            bindings: Vec::new(),
            enabled: true,
        }
    }

    /// Return a normalized copy suitable for persistence.
    #[must_use]
    pub fn normalized(&self) -> Self {
        let mut layer = self.clone();
        layer.opacity = normalize_f32(layer.opacity, 0.0, 1.0, default_layer_opacity());
        layer.transform = layer.transform.normalized();
        layer.adjust = layer.adjust.normalized();
        layer
    }

    /// Validate layer values that cannot be safely normalized.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        validate_finite(self.opacity, "opacity", &mut errors);
        self.source.validate(&mut errors);
        self.transform.validate(&mut errors);
        self.adjust.validate(&mut errors);
        for binding in &self.bindings {
            binding.validate(&mut errors);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Source that feeds one authored layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayerSource {
    /// A Hypercolor effect from the registry.
    Effect {
        effect_id: EffectId,
        #[serde(default)]
        controls: HashMap<String, ControlValue>,
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        control_bindings: HashMap<String, ControlBinding>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preset_id: Option<PresetId>,
    },

    /// A media asset from the asset library.
    Media {
        asset_id: AssetId,
        #[serde(default)]
        playback: MediaPlayback,
    },

    /// Live screen capture, full screen or sub-region.
    ScreenRegion {
        #[serde(default)]
        viewport: ViewportRect,
    },

    /// Arbitrary URL via the web viewport renderer.
    WebViewport {
        url: String,
        #[serde(default)]
        viewport: ViewportRect,
        #[serde(default)]
        render: WebViewportRender,
    },

    /// Constant color fill.
    ColorFill { rgba: [f32; 4] },
}

impl LayerSource {
    fn validate(&self, errors: &mut Vec<String>) {
        match self {
            Self::Media { playback, .. } => playback.validate(errors),
            Self::WebViewport { url, .. } if url.trim().is_empty() => {
                errors.push("web viewport url must not be empty".to_owned());
            }
            Self::ColorFill { rgba } => {
                for (index, channel) in rgba.iter().enumerate() {
                    let name = format!("source.rgba[{index}]");
                    validate_finite(*channel, &name, errors);
                    validate_range(*channel, 0.0..=1.0, &name, errors);
                }
            }
            Self::Effect { .. } | Self::ScreenRegion { .. } | Self::WebViewport { .. } => {}
        }
    }
}

/// Layer blend mode used by authored stacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerBlendMode {
    Replace,
    #[default]
    Alpha,
    Add,
    Screen,
    Multiply,
    Overlay,
    SoftLight,
    ColorDodge,
    Difference,
    Tint,
    LumaReveal,
}

impl LayerBlendMode {
    /// Return the equivalent canvas blend mode when one exists.
    #[must_use]
    pub const fn standard_canvas_blend_mode(self) -> Option<BlendMode> {
        match self {
            Self::Replace | Self::Tint | Self::LumaReveal => None,
            Self::Alpha => Some(BlendMode::Normal),
            Self::Add => Some(BlendMode::Add),
            Self::Screen => Some(BlendMode::Screen),
            Self::Multiply => Some(BlendMode::Multiply),
            Self::Overlay => Some(BlendMode::Overlay),
            Self::SoftLight => Some(BlendMode::SoftLight),
            Self::ColorDodge => Some(BlendMode::ColorDodge),
            Self::Difference => Some(BlendMode::Difference),
        }
    }
}

impl From<DisplayFaceBlendMode> for LayerBlendMode {
    fn from(value: DisplayFaceBlendMode) -> Self {
        match value {
            DisplayFaceBlendMode::Replace => Self::Replace,
            DisplayFaceBlendMode::Alpha => Self::Alpha,
            DisplayFaceBlendMode::Tint => Self::Tint,
            DisplayFaceBlendMode::LumaReveal => Self::LumaReveal,
            DisplayFaceBlendMode::Add => Self::Add,
            DisplayFaceBlendMode::Screen => Self::Screen,
            DisplayFaceBlendMode::Multiply => Self::Multiply,
            DisplayFaceBlendMode::Overlay => Self::Overlay,
            DisplayFaceBlendMode::SoftLight => Self::SoftLight,
            DisplayFaceBlendMode::ColorDodge => Self::ColorDodge,
            DisplayFaceBlendMode::Difference => Self::Difference,
        }
    }
}

/// Media playback settings for media-backed layers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaPlayback {
    #[serde(default = "default_playback_speed")]
    pub speed: f32,
    #[serde(default)]
    pub loop_mode: LoopMode,
    #[serde(default)]
    pub start_offset_secs: f32,
    #[serde(default = "default_true")]
    pub auto_play: bool,
}

impl Default for MediaPlayback {
    fn default() -> Self {
        Self {
            speed: default_playback_speed(),
            loop_mode: LoopMode::default(),
            start_offset_secs: 0.0,
            auto_play: true,
        }
    }
}

impl MediaPlayback {
    fn validate(&self, errors: &mut Vec<String>) {
        validate_finite(self.speed, "media.playback.speed", errors);
        validate_finite(
            self.start_offset_secs,
            "media.playback.start_offset_secs",
            errors,
        );
        if self.start_offset_secs.is_finite() && self.start_offset_secs < 0.0 {
            errors.push("media.playback.start_offset_secs must be non-negative".to_owned());
        }
    }
}

/// End-of-stream policy for media playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopMode {
    None,
    #[default]
    Loop,
    PingPong,
}

/// Web viewport render policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebViewportRender {
    #[default]
    Live,
    Snapshot,
}

/// Geometric placement for a layer source.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LayerTransform {
    pub anchor: NormalizedPosition,
    pub scale: [f32; 2],
    pub rotation: f32,
    pub fit: FitMode,
}

impl LayerTransform {
    /// Return a normalized copy suitable for persistence.
    #[must_use]
    pub fn normalized(self) -> Self {
        Self {
            anchor: NormalizedPosition::new(
                normalize_f32(self.anchor.x, 0.0, 1.0, 0.5),
                normalize_f32(self.anchor.y, 0.0, 1.0, 0.5),
            ),
            scale: [
                normalize_f32(self.scale[0], 0.01, 16.0, 1.0),
                normalize_f32(self.scale[1], 0.01, 16.0, 1.0),
            ],
            rotation: if self.rotation.is_finite() {
                self.rotation
            } else {
                0.0
            },
            fit: self.fit,
        }
    }

    fn validate(&self, errors: &mut Vec<String>) {
        validate_finite(self.anchor.x, "transform.anchor.x", errors);
        validate_finite(self.anchor.y, "transform.anchor.y", errors);
        validate_finite(self.scale[0], "transform.scale[0]", errors);
        validate_finite(self.scale[1], "transform.scale[1]", errors);
        validate_finite(self.rotation, "transform.rotation", errors);
        validate_range(self.scale[0], 0.01..=16.0, "transform.scale[0]", errors);
        validate_range(self.scale[1], 0.01..=16.0, "transform.scale[1]", errors);
    }
}

impl Default for LayerTransform {
    fn default() -> Self {
        Self {
            anchor: NormalizedPosition::new(0.5, 0.5),
            scale: [1.0, 1.0],
            rotation: 0.0,
            fit: FitMode::Cover,
        }
    }
}

/// Per-layer color adjustment settings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LayerAdjust {
    pub brightness: f32,
    pub saturation: f32,
    pub hue_shift: f32,
    pub tint: [f32; 4],
    pub tint_strength: f32,
    pub contrast: f32,
}

impl LayerAdjust {
    /// Return a normalized copy suitable for persistence.
    #[must_use]
    pub fn normalized(self) -> Self {
        Self {
            brightness: normalize_f32(self.brightness, 0.0, 4.0, 1.0),
            saturation: normalize_f32(self.saturation, 0.0, 4.0, 1.0),
            hue_shift: if self.hue_shift.is_finite() {
                self.hue_shift
            } else {
                0.0
            },
            tint: self
                .tint
                .map(|channel| normalize_f32(channel, 0.0, 1.0, 1.0)),
            tint_strength: normalize_f32(self.tint_strength, 0.0, 1.0, 0.0),
            contrast: normalize_f32(self.contrast, -1.0, 1.0, 0.0),
        }
    }

    fn validate(&self, errors: &mut Vec<String>) {
        validate_finite(self.brightness, "adjust.brightness", errors);
        validate_finite(self.saturation, "adjust.saturation", errors);
        validate_finite(self.hue_shift, "adjust.hue_shift", errors);
        validate_range(self.brightness, 0.0..=4.0, "adjust.brightness", errors);
        validate_range(self.saturation, 0.0..=4.0, "adjust.saturation", errors);
        for (index, channel) in self.tint.iter().enumerate() {
            let name = format!("adjust.tint[{index}]");
            validate_finite(*channel, &name, errors);
        }
        validate_finite(self.tint_strength, "adjust.tint_strength", errors);
        validate_finite(self.contrast, "adjust.contrast", errors);
        validate_range(
            self.tint_strength,
            0.0..=1.0,
            "adjust.tint_strength",
            errors,
        );
        validate_range(self.contrast, -1.0..=1.0, "adjust.contrast", errors);
    }
}

impl Default for LayerAdjust {
    fn default() -> Self {
        Self {
            brightness: 1.0,
            saturation: 1.0,
            hue_shift: 0.0,
            tint: [1.0, 1.0, 1.0, 1.0],
            tint_strength: 0.0,
            contrast: 0.0,
        }
    }
}

/// Live mapping from runtime data to a scalar layer parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerBinding {
    pub target: LayerParameter,
    pub source: BindingSource,
    pub map: BindingMap,
}

impl LayerBinding {
    fn validate(&self, errors: &mut Vec<String>) {
        self.source.validate(errors);
        self.map.validate(errors);
    }
}

/// Bindable scalar layer parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerParameter {
    Opacity,
    Brightness,
    Saturation,
    HueShift,
    TintStrength,
    Contrast,
    ScaleX,
    ScaleY,
    Rotation,
    PlaybackSpeed,
}

/// Runtime source that drives a layer binding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BindingSource {
    AudioBand { band: AudioBand },
    Sensor { name: String },
    Time { rate_hz: f32, wave: TimeWave },
    Constant { value: f32 },
}

impl BindingSource {
    fn validate(&self, errors: &mut Vec<String>) {
        match self {
            Self::Time { rate_hz, .. } => {
                validate_finite(*rate_hz, "binding.source.rate_hz", errors)
            }
            Self::Constant { value } => validate_finite(*value, "binding.source.value", errors),
            Self::AudioBand { .. } | Self::Sensor { .. } => {}
        }
    }
}

/// Coarse audio features exposed to layer bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioBand {
    Bass,
    Mid,
    Treble,
    Rms,
    Peak,
    BeatPulse,
    OnsetPulse,
}

/// Time-domain waveform for layer bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeWave {
    #[default]
    Sine,
    Triangle,
    Saw,
    Square,
}

/// Linear mapping from source values into target parameter values.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BindingMap {
    pub source_min: f32,
    pub source_max: f32,
    pub target_min: f32,
    pub target_max: f32,
    #[serde(default = "default_true")]
    pub clamp: bool,
}

impl BindingMap {
    /// Create a clamped linear source-to-target mapping.
    #[must_use]
    pub fn linear(source: RangeInclusive<f32>, target: RangeInclusive<f32>) -> Self {
        Self {
            source_min: *source.start(),
            source_max: *source.end(),
            target_min: *target.start(),
            target_max: *target.end(),
            clamp: true,
        }
    }

    fn validate(&self, errors: &mut Vec<String>) {
        validate_finite(self.source_min, "binding.map.source_min", errors);
        validate_finite(self.source_max, "binding.map.source_max", errors);
        validate_finite(self.target_min, "binding.map.target_min", errors);
        validate_finite(self.target_max, "binding.map.target_max", errors);
        if self.source_min == self.source_max {
            errors.push("binding source range must not be empty".to_owned());
        }
    }
}

impl Default for BindingMap {
    fn default() -> Self {
        Self::linear(0.0..=1.0, 0.0..=1.0)
    }
}

fn default_layer_opacity() -> f32 {
    1.0
}

fn default_playback_speed() -> f32 {
    1.0
}

fn default_true() -> bool {
    true
}

fn normalize_f32(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

fn validate_finite(value: f32, name: &str, errors: &mut Vec<String>) {
    if !value.is_finite() {
        errors.push(format!("{name} must be finite"));
    }
}

fn validate_range(value: f32, range: RangeInclusive<f32>, name: &str, errors: &mut Vec<String>) {
    if value.is_finite() && !range.contains(&value) {
        errors.push(format!(
            "{name} must be in [{}, {}]",
            range.start(),
            range.end()
        ));
    }
}
