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
//! macro asserts at compile time that no two topics claim the same
//! byte. Three tags are deliberately unowned and listed in
//! [`SHARED_TRANSPORT_TAGS`]: the wide passive preview frame, the
//! preview chunk, and the preview cancellation are transport envelopes
//! that four passive preview topics share, so no single topic can claim
//! them. Interactive preview is not a subscribable topic at all — it is
//! a session protocol keyed by preview id — so its two tags sit in the
//! same reserved list.

use serde::{Deserialize, Serialize};

use super::topic::{NoPatch, PatchError, TopicPatch};
use crate::define_ws_topics;

/// Frame delivery encoding for the `frames` topic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameFormat {
    /// Packed binary LED frames.
    Binary,
    /// JSON frame payloads.
    Json,
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
    /// Frame encoding.
    pub format: FrameFormat,
    /// Zone ids to deliver; `["all"]` selects every zone.
    pub zones: Vec<String>,
}

impl Default for FramesConfig {
    fn default() -> Self {
        Self {
            fps: 30,
            format: FrameFormat::Binary,
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
    /// Replacement encoding.
    #[serde(default)]
    pub format: Option<FrameFormat>,
    /// Replacement zone selection.
    #[serde(default)]
    pub zones: Option<Vec<String>>,
}

impl TopicPatch<FramesConfig> for FramesConfigPatch {
    fn apply(&self, config: &mut FramesConfig) -> Result<(), PatchError> {
        if let Some(fps) = self.fps {
            validate_range(fps, 1, 60, "fps", "expected 1..=60")?;
            config.fps = fps;
        }
        if let Some(format) = self.format {
            config.format = format;
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
            validate_range(fps, 1, 60, "fps", "expected 1..=60")?;
            config.fps = fps;
        }
        if let Some(bins) = self.bins {
            if ![8, 16, 32, 64, 128].contains(&bins) {
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
            validate_range(fps, 1, 60, "fps", "expected 1..=60")?;
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
                100,
                10_000,
                "interval_ms",
                "expected 100..=10000",
            )?;
            config.interval_ms = interval_ms;
        }
        Ok(())
    }
}

/// Per-subscription configuration for the `display_preview` topic.
/// `device_id` stays `None` until a client names a target; clearing it
/// detaches the relay from the device's frame stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayPreviewConfig {
    /// Target device, or `None` while the subscription is detached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// Delivery cadence in frames per second.
    pub fps: u32,
}

impl Default for DisplayPreviewConfig {
    fn default() -> Self {
        Self {
            device_id: None,
            fps: 15,
        }
    }
}

/// Patch for [`DisplayPreviewConfig`]. `device_id` is a double-`Option`
/// because the three client intents are distinct on the wire: an absent
/// key leaves the target alone, `null` clears it, and a string sets it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayPreviewConfigPatch {
    /// Tri-state target update.
    #[serde(default, deserialize_with = "deserialize_double_option_string")]
    #[allow(
        clippy::option_option,
        reason = "the patch protocol needs distinct states for missing, null, and string values"
    )]
    pub device_id: Option<Option<String>>,
    /// Replacement cadence.
    #[serde(default)]
    pub fps: Option<u32>,
}

impl TopicPatch<DisplayPreviewConfig> for DisplayPreviewConfigPatch {
    fn apply(&self, config: &mut DisplayPreviewConfig) -> Result<(), PatchError> {
        if let Some(device_id) = self.device_id.clone() {
            match device_id {
                Some(id) => {
                    // Trim so accidental whitespace cannot sneak a
                    // subscription through with no real device behind it.
                    let trimmed = id.trim();
                    if trimmed.is_empty() {
                        return Err(PatchError::new(
                            "device_id",
                            "must be non-empty when provided",
                        ));
                    }
                    config.device_id = Some(trimmed.to_owned());
                }
                None => config.device_id = None,
            }
        }
        if let Some(fps) = self.fps {
            validate_range(fps, 1, 30, "fps", "expected 1..=30")?;
            config.fps = fps;
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

/// Deserialize a double-`Option` so `null` maps to `Some(None)` (explicit
/// clear) and a missing key keeps the outer `None` through
/// `#[serde(default)]`. Serde's own behavior collapses both into `None`.
#[allow(
    clippy::option_option,
    reason = "serde needs the tri-state shape to preserve missing-vs-null during patch application"
)]
fn deserialize_double_option_string<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

define_ws_topics! {
    registry TopicId;

    topic Frames => "frames" {
        key: unkeyed, config: FramesConfig, patch: FramesConfigPatch,
        tags: [0x01], control: false,
    }
    topic Spectrum => "spectrum" {
        key: unkeyed, config: SpectrumConfig, patch: SpectrumConfigPatch,
        tags: [0x02], control: false,
    }
    topic Events => "events" {
        key: unkeyed, config: (), patch: NoPatch,
        tags: [], control: false,
    }
    topic FrameEvents => "frame_events" {
        key: unkeyed, config: (), patch: NoPatch,
        tags: [], control: false,
    }
    topic Canvas => "canvas" {
        key: unkeyed, config: CanvasConfig, patch: CanvasConfigPatch,
        tags: [0x03], control: false,
    }
    topic ScreenCanvas => "screen_canvas" {
        key: unkeyed, config: CanvasConfig, patch: CanvasConfigPatch,
        tags: [0x05], control: true,
    }
    topic ScreenZones => "screen_zones" {
        key: unkeyed, config: (), patch: NoPatch,
        tags: [0x09, 0x0e, 0x11], control: true,
    }
    topic WebViewportCanvas => "web_viewport_canvas" {
        key: unkeyed, config: CanvasConfig, patch: CanvasConfigPatch,
        tags: [0x06], control: false,
    }
    topic ZonePreview => "zone_preview" {
        key: unkeyed, config: CanvasConfig, patch: CanvasConfigPatch,
        tags: [0x08, 0x0c], control: false,
    }
    topic Metrics => "metrics" {
        key: unkeyed, config: MetricsConfig, patch: MetricsConfigPatch,
        tags: [], control: false,
    }
    topic DeviceMetrics => "device_metrics" {
        key: unkeyed, config: MetricsConfig, patch: MetricsConfigPatch,
        tags: [], control: false,
    }
    topic Sensors => "sensors" {
        key: unkeyed, config: (), patch: NoPatch,
        tags: [], control: false,
    }
    topic DisplayPreview => "display_preview" {
        key: unkeyed, config: DisplayPreviewConfig, patch: DisplayPreviewConfigPatch,
        tags: [0x07], control: false,
    }
    topic InputEvents => "input_events" {
        key: unkeyed, config: (), patch: NoPatch,
        tags: [], control: true,
    }
}

/// Binary tags no single topic owns.
///
/// `0x0b` is the wide form of the passive preview frame, which four
/// topics publish; `0x0f` and `0x10` are the chunk and cancellation
/// envelopes every preview stream rides. `0x0a` and `0x0d` belong to
/// interactive preview, a keyed session protocol rather than a
/// subscribable topic.
pub const SHARED_TRANSPORT_TAGS: &[u8] = &[0x0a, 0x0b, 0x0d, 0x0f, 0x10];

// The shared tags are as exclusive as the owned ones: a topic quietly
// claiming a transport envelope byte would collide on the wire without
// the per-topic assertion ever noticing.
const _: () = {
    assert!(
        super::topic::tags_disjoint(&[
            SHARED_TRANSPORT_TAGS,
            <Frames as super::topic::WsTopic>::OWNED_TAGS,
            <Spectrum as super::topic::WsTopic>::OWNED_TAGS,
            <Canvas as super::topic::WsTopic>::OWNED_TAGS,
            <ScreenCanvas as super::topic::WsTopic>::OWNED_TAGS,
            <ScreenZones as super::topic::WsTopic>::OWNED_TAGS,
            <WebViewportCanvas as super::topic::WsTopic>::OWNED_TAGS,
            <ZonePreview as super::topic::WsTopic>::OWNED_TAGS,
            <DisplayPreview as super::topic::WsTopic>::OWNED_TAGS,
        ]),
        "a topic claims a shared preview transport tag"
    );
};
