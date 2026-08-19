//! The live WebSocket topic topology (Spec 76 §5).
//!
//! Every subscribable topic on `/api/v1/ws` is declared once here, with
//! its wire name, key shape, config, patch, owned binary tags, and
//! control-tier gate. The daemon reads this registry instead of keeping
//! its own parallel channel enum, bitset, and config struct, so a wire
//! fact has exactly one home.
//!
//! What lives here is what the wire agrees on. Which bus lane feeds a
//! topic, which relay task serves it, and what engine demand a
//! subscription implies are runtime facts, and they stay in the daemon's
//! own table keyed by [`TopicId`].
//!
//! # Tag ownership
//!
//! A topic owns the binary tags only its own codec writes, and the
//! macro asserts at compile time that no two topics claim the same byte
//! and that none of them claims a reserved one. Three tags are
//! deliberately unowned and sit in the macro's `reserved` clause: the
//! wide passive preview frame, the preview chunk, and the preview
//! cancellation are transport envelopes that several topics share, so no
//! single topic can claim them.
//!
//! # Keys
//!
//! Two topics are keyed. `display_preview` is keyed by device, so one
//! connection can follow several displays at once and each frame names
//! the device it came from. `interactive_preview` is keyed by the
//! client's preview id: subscribing opens a render lane and
//! unsubscribing closes it, which is why it is a control-tier topic.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::preview::{DISPLAY_PREVIEW_ID_MAX_BYTES, INTERACTIVE_PREVIEW_ID_MAX_BYTES};
use super::topic::{KeyError, NoPatch, PatchError, TopicKey, TopicPatch};
use crate::define_ws_topics;

const STANDARD_FPS_MIN: u32 = 1;
const STANDARD_FPS_MAX: u32 = 60;
const DISPLAY_PREVIEW_FPS_MAX: u32 = 30;
const SPECTRUM_BIN_COUNTS: [u16; 5] = [8, 16, 32, 64, 128];

fn no_config_schema() -> Value {
    Value::Null
}

fn frames_config_schema() -> Value {
    json!({
        "fps": {"min": STANDARD_FPS_MIN, "max": STANDARD_FPS_MAX},
        "zones": {"min_items": 1},
    })
}

fn spectrum_config_schema() -> Value {
    json!({
        "fps": {"min": STANDARD_FPS_MIN, "max": STANDARD_FPS_MAX},
        "bins": {"values": SPECTRUM_BIN_COUNTS},
    })
}

fn screen_zones_config_schema() -> Value {
    json!({
        "fps": {"min": STANDARD_FPS_MIN, "max": STANDARD_FPS_MAX},
    })
}

fn canvas_config_schema() -> Value {
    json!({
        "fps": {"min": STANDARD_FPS_MIN, "max": STANDARD_FPS_MAX},
        "format": {"values": ["rgb", "rgba", "jpeg"]},
        "width": {"min": 0},
        "height": {"min": 0},
    })
}

fn metrics_config_schema() -> Value {
    json!({
        "interval_ms": {
            "min": METRICS_INTERVAL_MS_MIN,
            "max": METRICS_INTERVAL_MS_MAX,
        },
    })
}

fn display_preview_config_schema() -> Value {
    json!({
        "fps": {"min": STANDARD_FPS_MIN, "max": DISPLAY_PREVIEW_FPS_MAX},
    })
}

fn interactive_preview_config_schema() -> Value {
    json!({
        "target": {"values": ["active_scene"]},
        "fps": {"min": STANDARD_FPS_MIN, "max": STANDARD_FPS_MAX},
        "width": {"min": 1},
        "height": {"min": 1},
        "format": {"values": ["rgb", "rgba", "jpeg"]},
    })
}

/// The `display_preview` subscription key: the device whose display
/// surface this subscription follows. One connection holds as many
/// display previews as it names distinct devices.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeviceKey(String);

impl DeviceKey {
    /// Build a key from a device id, validating it the way the wire does.
    pub fn new(device_id: impl Into<String>) -> Result<Self, KeyError> {
        let device_id = device_id.into();
        let trimmed = device_id.trim();
        validate_identity(trimmed, DISPLAY_PREVIEW_ID_MAX_BYTES, "device id")?;
        Ok(Self(trimmed.to_owned()))
    }

    /// The device id this subscription follows.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TopicKey for DeviceKey {
    const WIRE_NAME: Option<&'static str> = Some("device_id");

    fn to_wire(&self) -> Option<String> {
        Some(self.0.clone())
    }

    fn from_wire(key: Option<&str>) -> Result<Self, KeyError> {
        Self::new(key.ok_or(KeyError::MissingKey)?)
    }
}

/// The `interactive_preview` subscription key: the client-chosen preview
/// id. It also identifies the preview's binary frames and every message
/// addressed at one live preview.
///
/// Unlike a device id, a preview id is opaque to the daemon, so it is
/// taken verbatim rather than trimmed: two ids differing only in padding
/// are two previews, which is the client's business and not ours.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PreviewKey(String);

impl PreviewKey {
    /// Build a key from a preview id, validating it the way the wire does.
    pub fn new(preview_id: impl Into<String>) -> Result<Self, KeyError> {
        let preview_id = preview_id.into();
        validate_identity(&preview_id, INTERACTIVE_PREVIEW_ID_MAX_BYTES, "preview id")?;
        Ok(Self(preview_id))
    }

    /// The preview id this subscription drives.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TopicKey for PreviewKey {
    const WIRE_NAME: Option<&'static str> = Some("preview_id");

    fn to_wire(&self) -> Option<String> {
        Some(self.0.clone())
    }

    fn from_wire(key: Option<&str>) -> Result<Self, KeyError> {
        Self::new(key.ok_or(KeyError::MissingKey)?)
    }
}

/// Both keyed topics carry a client-supplied identity that ends up in a
/// binary frame header behind a one-byte length, so the same three rules
/// apply to each: non-empty, bounded, and free of control characters.
fn validate_identity(value: &str, max_bytes: usize, subject: &str) -> Result<(), KeyError> {
    if value.is_empty() {
        return Err(KeyError::Invalid(format!("{subject} must not be empty")));
    }
    if value.len() > max_bytes {
        return Err(KeyError::Invalid(format!(
            "{subject} cannot exceed {max_bytes} UTF-8 bytes"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(KeyError::Invalid(format!(
            "{subject} cannot contain control characters"
        )));
    }
    Ok(())
}

/// Pixel encoding for the passive preview canvas topics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanvasFormat {
    /// Three bytes per pixel.
    Rgb,
    /// Four bytes per pixel.
    Rgba,
    /// JPEG-compressed frames.
    Jpeg,
}

/// Per-subscription configuration for the `frames` topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FramesConfig {
    /// Delivery cadence in frames per second.
    pub fps: u32,
    /// Zone ids to deliver; `["all"]` selects every zone.
    pub zones: Vec<String>,
}

impl Default for FramesConfig {
    fn default() -> Self {
        Self {
            fps: 30,
            zones: vec!["all".to_owned()],
        }
    }
}

/// Patch for [`FramesConfig`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FramesConfigPatch {
    /// Replacement cadence.
    #[serde(default)]
    pub fps: Option<u32>,
    /// Replacement zone selection.
    #[serde(default)]
    pub zones: Option<Vec<String>>,
}

impl TopicPatch<FramesConfig> for FramesConfigPatch {
    fn apply(&self, config: &mut FramesConfig) -> Result<(), PatchError> {
        if let Some(fps) = self.fps {
            validate_range(
                fps,
                STANDARD_FPS_MIN,
                STANDARD_FPS_MAX,
                "fps",
                "expected 1..=60",
            )?;
            config.fps = fps;
        }
        if let Some(zones) = self.zones.clone() {
            if zones.is_empty() {
                return Err(PatchError::new("zones", "must not be empty"));
            }
            config.zones = zones;
        }
        Ok(())
    }
}

/// Per-subscription configuration for the `spectrum` topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpectrumConfig {
    /// Delivery cadence in frames per second.
    pub fps: u32,
    /// FFT bin count.
    pub bins: u16,
}

impl Default for SpectrumConfig {
    fn default() -> Self {
        Self { fps: 30, bins: 64 }
    }
}

/// Patch for [`SpectrumConfig`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpectrumConfigPatch {
    /// Replacement cadence.
    #[serde(default)]
    pub fps: Option<u32>,
    /// Replacement bin count.
    #[serde(default)]
    pub bins: Option<u16>,
}

impl TopicPatch<SpectrumConfig> for SpectrumConfigPatch {
    fn apply(&self, config: &mut SpectrumConfig) -> Result<(), PatchError> {
        if let Some(fps) = self.fps {
            validate_range(
                fps,
                STANDARD_FPS_MIN,
                STANDARD_FPS_MAX,
                "fps",
                "expected 1..=60",
            )?;
            config.fps = fps;
        }
        if let Some(bins) = self.bins {
            if !SPECTRUM_BIN_COUNTS.contains(&bins) {
                return Err(PatchError::new(
                    "bins",
                    "expected one of [8, 16, 32, 64, 128]",
                ));
            }
            config.bins = bins;
        }
        Ok(())
    }
}

/// Per-subscription configuration for the `screen_zones` topic.
///
/// The topic used to be configless and was paced by `screen_canvas`'s
/// cadence — a value a `screen_zones`-only subscriber never set, on a
/// topic it was not subscribed to (Spec 78 §7.1). It owns its own now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenZonesConfig {
    /// Delivery cadence in frames per second.
    pub fps: u32,
}

impl Default for ScreenZonesConfig {
    fn default() -> Self {
        // The cadence it effectively ran at while borrowing
        // `screen_canvas`'s config. Owning the field changes who decides
        // the rate, not the rate itself.
        Self { fps: 15 }
    }
}

/// Patch for [`ScreenZonesConfig`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenZonesConfigPatch {
    /// Replacement cadence.
    #[serde(default)]
    pub fps: Option<u32>,
}

impl TopicPatch<ScreenZonesConfig> for ScreenZonesConfigPatch {
    fn apply(&self, config: &mut ScreenZonesConfig) -> Result<(), PatchError> {
        if let Some(fps) = self.fps {
            validate_range(
                fps,
                STANDARD_FPS_MIN,
                STANDARD_FPS_MAX,
                "fps",
                "expected 1..=60",
            )?;
            config.fps = fps;
        }
        Ok(())
    }
}

/// Per-subscription configuration shared by the passive preview canvas
/// topics. `width` and `height` of zero mean "server picks", which is
/// why neither carries an upper bound here: the admissible surface size
/// is a runtime resource question the daemon answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanvasConfig {
    /// Delivery cadence in frames per second.
    pub fps: u32,
    /// Pixel encoding.
    pub format: CanvasFormat,
    /// Requested width, or zero for the server default.
    pub width: u32,
    /// Requested height, or zero for the server default.
    pub height: u32,
}

impl Default for CanvasConfig {
    fn default() -> Self {
        Self {
            fps: 15,
            format: CanvasFormat::Rgb,
            width: 0,
            height: 0,
        }
    }
}

/// Patch for [`CanvasConfig`].
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanvasConfigPatch {
    /// Replacement cadence.
    #[serde(default)]
    pub fps: Option<u32>,
    /// Replacement pixel encoding.
    #[serde(default)]
    pub format: Option<CanvasFormat>,
    /// Replacement width.
    #[serde(default)]
    pub width: Option<u32>,
    /// Replacement height.
    #[serde(default)]
    pub height: Option<u32>,
}

impl TopicPatch<CanvasConfig> for CanvasConfigPatch {
    fn apply(&self, config: &mut CanvasConfig) -> Result<(), PatchError> {
        if let Some(fps) = self.fps {
            validate_range(
                fps,
                STANDARD_FPS_MIN,
                STANDARD_FPS_MAX,
                "fps",
                "expected 1..=60",
            )?;
            config.fps = fps;
        }
        if let Some(format) = self.format {
            config.format = format;
        }
        if let Some(width) = self.width {
            config.width = width;
        }
        if let Some(height) = self.height {
            config.height = height;
        }
        Ok(())
    }
}

/// Per-subscription configuration for the periodic telemetry topics.
pub const METRICS_INTERVAL_MS_MIN: u32 = 100;
/// Largest supported telemetry snapshot period.
pub const METRICS_INTERVAL_MS_MAX: u32 = 10_000;

/// Per-subscription configuration for the periodic telemetry topics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricsConfig {
    /// Snapshot period in milliseconds.
    pub interval_ms: u32,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self { interval_ms: 1000 }
    }
}

/// Patch for [`MetricsConfig`].
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricsConfigPatch {
    /// Replacement snapshot period.
    #[serde(default)]
    pub interval_ms: Option<u32>,
}

impl TopicPatch<MetricsConfig> for MetricsConfigPatch {
    fn apply(&self, config: &mut MetricsConfig) -> Result<(), PatchError> {
        if let Some(interval_ms) = self.interval_ms {
            validate_range(
                interval_ms,
                METRICS_INTERVAL_MS_MIN,
                METRICS_INTERVAL_MS_MAX,
                "interval_ms",
                "expected 100..=10000",
            )?;
            config.interval_ms = interval_ms;
        }
        Ok(())
    }
}

/// Per-subscription configuration for the `display_preview` topic. The
/// target device is the subscription's key, not a config field, so a
/// connection following three displays holds three subscriptions rather
/// than one slot it keeps retargeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayPreviewConfig {
    /// Delivery cadence in frames per second.
    pub fps: u32,
}

impl Default for DisplayPreviewConfig {
    fn default() -> Self {
        Self { fps: 15 }
    }
}

/// Patch for [`DisplayPreviewConfig`].
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayPreviewConfigPatch {
    /// Replacement cadence.
    #[serde(default)]
    pub fps: Option<u32>,
}

impl TopicPatch<DisplayPreviewConfig> for DisplayPreviewConfigPatch {
    fn apply(&self, config: &mut DisplayPreviewConfig) -> Result<(), PatchError> {
        if let Some(fps) = self.fps {
            validate_range(
                fps,
                STANDARD_FPS_MIN,
                DISPLAY_PREVIEW_FPS_MAX,
                "fps",
                "expected 1..=30",
            )?;
            config.fps = fps;
        }
        Ok(())
    }
}

/// What an interactive preview renders.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractivePreviewTarget {
    /// The scene currently active on the daemon.
    #[default]
    ActiveScene,
}

/// Per-subscription configuration for the `interactive_preview` topic.
///
/// Unlike the passive preview surfaces, an interactive preview has no
/// server-picked size: the daemon opens a render lane at exactly the
/// requested shape, so zero dimensions are rejected rather than resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractivePreviewConfig {
    /// What the preview renders.
    pub target: InteractivePreviewTarget,
    /// Delivery cadence in frames per second.
    pub fps: u32,
    /// Render width in pixels.
    pub width: u32,
    /// Render height in pixels.
    pub height: u32,
    /// Pixel encoding.
    pub format: CanvasFormat,
}

impl Default for InteractivePreviewConfig {
    fn default() -> Self {
        Self {
            target: InteractivePreviewTarget::ActiveScene,
            fps: 30,
            width: DEFAULT_INTERACTIVE_PREVIEW_WIDTH,
            height: DEFAULT_INTERACTIVE_PREVIEW_HEIGHT,
            format: CanvasFormat::Jpeg,
        }
    }
}

/// Default interactive preview width when a client subscribes without one.
pub const DEFAULT_INTERACTIVE_PREVIEW_WIDTH: u32 = 640;
/// Default interactive preview height when a client subscribes without one.
pub const DEFAULT_INTERACTIVE_PREVIEW_HEIGHT: u32 = 480;

/// Patch for [`InteractivePreviewConfig`].
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractivePreviewConfigPatch {
    /// Replacement target.
    #[serde(default)]
    pub target: Option<InteractivePreviewTarget>,
    /// Replacement cadence.
    #[serde(default)]
    pub fps: Option<u32>,
    /// Replacement render width.
    #[serde(default)]
    pub width: Option<u32>,
    /// Replacement render height.
    #[serde(default)]
    pub height: Option<u32>,
    /// Replacement pixel encoding.
    #[serde(default)]
    pub format: Option<CanvasFormat>,
}

impl TopicPatch<InteractivePreviewConfig> for InteractivePreviewConfigPatch {
    fn apply(&self, config: &mut InteractivePreviewConfig) -> Result<(), PatchError> {
        if let Some(target) = self.target {
            config.target = target;
        }
        if let Some(fps) = self.fps {
            validate_range(
                fps,
                STANDARD_FPS_MIN,
                STANDARD_FPS_MAX,
                "fps",
                "expected 1..=60",
            )?;
            config.fps = fps;
        }
        if let Some(width) = self.width {
            if width == 0 {
                return Err(PatchError::new("width", "must be non-zero"));
            }
            config.width = width;
        }
        if let Some(height) = self.height {
            if height == 0 {
                return Err(PatchError::new("height", "must be non-zero"));
            }
            config.height = height;
        }
        if let Some(format) = self.format {
            config.format = format;
        }
        Ok(())
    }
}

fn validate_range(
    value: u32,
    min: u32,
    max: u32,
    field: &'static str,
    reason: &'static str,
) -> Result<(), PatchError> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(PatchError::new(field, reason))
    }
}

define_ws_topics! {
    registry TopicId;

    // Transport envelopes no single topic owns: `0x0b` is the wide form
    // of the passive preview frame that four topics publish, and `0x0f`
    // and `0x10` are the chunk and cancellation envelopes every preview
    // stream rides. `0x04` stays deliberately unassigned.
    reserved [0x0b, 0x0f, 0x10];

    topic Frames => "frames" {
        key: unkeyed, config: FramesConfig, patch: FramesConfigPatch,
        schema: frames_config_schema,
        tags: [0x01], control: false,
        backpressure: DropWithNotice,
    }
    topic Spectrum => "spectrum" {
        key: unkeyed, config: SpectrumConfig, patch: SpectrumConfigPatch,
        schema: spectrum_config_schema,
        tags: [0x02], control: false,
        backpressure: DropWithNotice,
    }
    topic Events => "events" {
        key: unkeyed, config: (), patch: NoPatch, schema: no_config_schema,
        tags: [], control: false,
        backpressure: Lossless,
    }
    topic FrameEvents => "frame_events" {
        key: unkeyed, config: (), patch: NoPatch, schema: no_config_schema,
        tags: [], control: false,
        backpressure: Lossless,
    }
    topic Canvas => "canvas" {
        key: unkeyed, config: CanvasConfig, patch: CanvasConfigPatch,
        schema: canvas_config_schema,
        tags: [0x03], control: false,
        backpressure: LatestWins,
    }
    topic ScreenCanvas => "screen_canvas" {
        key: unkeyed, config: CanvasConfig, patch: CanvasConfigPatch,
        schema: canvas_config_schema,
        tags: [0x05], control: true,
        backpressure: LatestWins,
    }
    topic ScreenZones => "screen_zones" {
        key: unkeyed, config: ScreenZonesConfig, patch: ScreenZonesConfigPatch,
        schema: screen_zones_config_schema,
        tags: [0x09, 0x0e, 0x11], control: true,
        backpressure: LatestWins,
    }
    topic WebViewportCanvas => "web_viewport_canvas" {
        key: unkeyed, config: CanvasConfig, patch: CanvasConfigPatch,
        schema: canvas_config_schema,
        tags: [0x06], control: false,
        backpressure: LatestWins,
    }
    topic ZonePreview => "zone_preview" {
        key: unkeyed, config: CanvasConfig, patch: CanvasConfigPatch,
        schema: canvas_config_schema,
        tags: [0x08, 0x0c], control: false,
        backpressure: LatestWins,
    }
    topic Metrics => "metrics" {
        key: unkeyed, config: MetricsConfig, patch: MetricsConfigPatch,
        schema: metrics_config_schema,
        tags: [], control: false,
        backpressure: DropWithNotice,
    }
    topic DeviceMetrics => "device_metrics" {
        key: unkeyed, config: MetricsConfig, patch: MetricsConfigPatch,
        schema: metrics_config_schema,
        tags: [], control: false,
        backpressure: DropWithNotice,
    }
    topic Sensors => "sensors" {
        key: unkeyed, config: (), patch: NoPatch, schema: no_config_schema,
        tags: [], control: false,
        backpressure: LatestWins,
    }
    topic DisplayPreview => "display_preview" {
        key: DeviceKey, config: DisplayPreviewConfig, patch: DisplayPreviewConfigPatch,
        schema: display_preview_config_schema,
        tags: [0x07, 0x12], control: false,
        backpressure: LatestWins,
    }
    topic InteractivePreview => "interactive_preview" {
        key: PreviewKey, config: InteractivePreviewConfig, patch: InteractivePreviewConfigPatch,
        schema: interactive_preview_config_schema,
        tags: [0x0a, 0x0d], control: true,
        backpressure: LatestWins,
    }
    topic InputEvents => "input_events" {
        key: unkeyed, config: (), patch: NoPatch, schema: no_config_schema,
        tags: [], control: true,
        backpressure: Lossless,
    }
}
