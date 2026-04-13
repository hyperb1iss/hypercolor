//! WebSocket protocol types — subscriptions, configs, and client/server messages.
//!
//! These types describe the wire format on `/api/v1/ws`. Everything here is data —
//! no network I/O, no caches, no runtime state.

use std::collections::HashSet;
use std::hash::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use serde_json::json;

use hypercolor_types::server::ServerIdentity;

// ── Subscription Types ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum WsChannel {
    Frames,
    Spectrum,
    Events,
    Canvas,
    ScreenCanvas,
    WebViewportCanvas,
    Metrics,
    DisplayPreview,
}

impl WsChannel {
    pub(super) const SUPPORTED: [Self; 8] = [
        Self::Frames,
        Self::Spectrum,
        Self::Events,
        Self::Canvas,
        Self::ScreenCanvas,
        Self::WebViewportCanvas,
        Self::Metrics,
        Self::DisplayPreview,
    ];

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Frames => "frames",
            Self::Spectrum => "spectrum",
            Self::Events => "events",
            Self::Canvas => "canvas",
            Self::ScreenCanvas => "screen_canvas",
            Self::WebViewportCanvas => "web_viewport_canvas",
            Self::Metrics => "metrics",
            Self::DisplayPreview => "display_preview",
        }
    }

    pub(super) fn parse(raw: &str) -> Option<Self> {
        match raw {
            "frames" => Some(Self::Frames),
            "spectrum" => Some(Self::Spectrum),
            "events" => Some(Self::Events),
            "canvas" => Some(Self::Canvas),
            "screen_canvas" => Some(Self::ScreenCanvas),
            "web_viewport_canvas" => Some(Self::WebViewportCanvas),
            "metrics" => Some(Self::Metrics),
            "display_preview" => Some(Self::DisplayPreview),
            _ => None,
        }
    }

    pub(super) fn is_supported(self) -> bool {
        Self::SUPPORTED.contains(&self)
    }

    const fn bit(self) -> u8 {
        match self {
            Self::Frames => 1 << 0,
            Self::Spectrum => 1 << 1,
            Self::Events => 1 << 2,
            Self::Canvas => 1 << 3,
            Self::ScreenCanvas => 1 << 4,
            Self::WebViewportCanvas => 1 << 5,
            Self::Metrics => 1 << 6,
            Self::DisplayPreview => 1 << 7,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ChannelSet(u8);

impl ChannelSet {
    pub(super) const fn contains(self, channel: WsChannel) -> bool {
        self.0 & channel.bit() != 0
    }

    pub(super) fn insert(&mut self, channel: WsChannel) {
        self.0 |= channel.bit();
    }

    pub(super) fn remove(&mut self, channel: WsChannel) {
        self.0 &= !channel.bit();
    }

    pub(super) fn iter(self) -> impl Iterator<Item = WsChannel> {
        WsChannel::SUPPORTED
            .into_iter()
            .filter(move |channel| self.contains(*channel))
    }

    pub(super) fn from_channels(channels: &[WsChannel]) -> Self {
        let mut set = Self::default();
        for channel in channels {
            set.insert(*channel);
        }
        set
    }
}

#[derive(Debug, Clone)]
pub(super) struct SubscriptionState {
    pub(super) channels: ChannelSet,
    pub(super) config: ChannelConfig,
}

impl Default for SubscriptionState {
    fn default() -> Self {
        let mut channels = ChannelSet::default();
        channels.insert(WsChannel::Events);
        Self {
            channels,
            config: ChannelConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub(super) struct ChannelConfig {
    pub(super) frames: FramesConfig,
    pub(super) spectrum: SpectrumConfig,
    pub(super) canvas: CanvasConfig,
    pub(super) screen_canvas: CanvasConfig,
    pub(super) web_viewport_canvas: CanvasConfig,
    pub(super) metrics: MetricsConfig,
    pub(super) display_preview: DisplayPreviewConfig,
}

impl ChannelConfig {
    pub(super) fn apply_patch(&mut self, patch: ChannelConfigPatch) -> Result<(), WsProtocolError> {
        if let Some(frames) = patch.frames {
            if let Some(fps) = frames.fps {
                validate_range(fps, 1, 60, "config.frames.fps", "expected 1..=60")?;
                self.frames.fps = fps;
            }
            if let Some(format) = frames.format {
                self.frames.format = format;
            }
            if let Some(zones) = frames.zones {
                if zones.is_empty() {
                    return Err(WsProtocolError::invalid_config(
                        "config.frames.zones",
                        "must not be empty",
                    ));
                }
                self.frames.zones = zones;
            }
        }

        if let Some(spectrum) = patch.spectrum {
            if let Some(fps) = spectrum.fps {
                validate_range(fps, 1, 60, "config.spectrum.fps", "expected 1..=60")?;
                self.spectrum.fps = fps;
            }
            if let Some(bins) = spectrum.bins {
                if ![8, 16, 32, 64, 128].contains(&bins) {
                    return Err(WsProtocolError::invalid_config(
                        "config.spectrum.bins",
                        "expected one of [8, 16, 32, 64, 128]",
                    ));
                }
                self.spectrum.bins = bins;
            }
        }

        if let Some(canvas) = patch.canvas {
            if let Some(fps) = canvas.fps {
                validate_range(fps, 1, 60, "config.canvas.fps", "expected 1..=60")?;
                self.canvas.fps = fps;
            }
            if let Some(format) = canvas.format {
                self.canvas.format = format;
            }
            if let Some(width) = canvas.width {
                validate_range(width, 0, 4096, "config.canvas.width", "expected 0..=4096")?;
                self.canvas.width = width;
            }
            if let Some(height) = canvas.height {
                validate_range(height, 0, 4096, "config.canvas.height", "expected 0..=4096")?;
                self.canvas.height = height;
            }
        }

        if let Some(screen_canvas) = patch.screen_canvas {
            if let Some(fps) = screen_canvas.fps {
                validate_range(fps, 1, 60, "config.screen_canvas.fps", "expected 1..=60")?;
                self.screen_canvas.fps = fps;
            }
            if let Some(format) = screen_canvas.format {
                self.screen_canvas.format = format;
            }
            if let Some(width) = screen_canvas.width {
                validate_range(
                    width,
                    0,
                    4096,
                    "config.screen_canvas.width",
                    "expected 0..=4096",
                )?;
                self.screen_canvas.width = width;
            }
            if let Some(height) = screen_canvas.height {
                validate_range(
                    height,
                    0,
                    4096,
                    "config.screen_canvas.height",
                    "expected 0..=4096",
                )?;
                self.screen_canvas.height = height;
            }
        }

        if let Some(web_viewport_canvas) = patch.web_viewport_canvas {
            if let Some(fps) = web_viewport_canvas.fps {
                validate_range(
                    fps,
                    1,
                    60,
                    "config.web_viewport_canvas.fps",
                    "expected 1..=60",
                )?;
                self.web_viewport_canvas.fps = fps;
            }
            if let Some(format) = web_viewport_canvas.format {
                self.web_viewport_canvas.format = format;
            }
            if let Some(width) = web_viewport_canvas.width {
                validate_range(
                    width,
                    0,
                    4096,
                    "config.web_viewport_canvas.width",
                    "expected 0..=4096",
                )?;
                self.web_viewport_canvas.width = width;
            }
            if let Some(height) = web_viewport_canvas.height {
                validate_range(
                    height,
                    0,
                    4096,
                    "config.web_viewport_canvas.height",
                    "expected 0..=4096",
                )?;
                self.web_viewport_canvas.height = height;
            }
        }

        if let Some(metrics) = patch.metrics
            && let Some(interval_ms) = metrics.interval_ms
        {
            validate_range(
                interval_ms,
                100,
                10_000,
                "config.metrics.interval_ms",
                "expected 100..=10000",
            )?;
            self.metrics.interval_ms = interval_ms;
        }

        if let Some(display_preview) = patch.display_preview {
            // Double-Option: outer `Some` means the client sent the key;
            // inner `None` explicitly clears the target (disabling the
            // relay). Trim non-empty strings so accidental whitespace
            // doesn't sneak a subscription through with no real device.
            if let Some(device_id) = display_preview.device_id {
                match device_id {
                    Some(id) => {
                        let trimmed = id.trim();
                        if trimmed.is_empty() {
                            return Err(WsProtocolError::invalid_config(
                                "config.display_preview.device_id",
                                "must be non-empty when provided",
                            ));
                        }
                        self.display_preview.device_id = Some(trimmed.to_owned());
                    }
                    None => self.display_preview.device_id = None,
                }
            }
            if let Some(fps) = display_preview.fps {
                validate_range(fps, 1, 30, "config.display_preview.fps", "expected 1..=30")?;
                self.display_preview.fps = fps;
            }
        }

        Ok(())
    }

    pub(super) fn filtered_json(&self, channels: ChannelSet) -> serde_json::Value {
        let mut map = serde_json::Map::new();

        for channel in channels.iter() {
            let value = match channel {
                WsChannel::Frames => serde_json::to_value(&self.frames),
                WsChannel::Spectrum => serde_json::to_value(&self.spectrum),
                WsChannel::Canvas => serde_json::to_value(&self.canvas),
                WsChannel::ScreenCanvas => serde_json::to_value(&self.screen_canvas),
                WsChannel::WebViewportCanvas => serde_json::to_value(&self.web_viewport_canvas),
                WsChannel::Metrics => serde_json::to_value(&self.metrics),
                WsChannel::DisplayPreview => serde_json::to_value(&self.display_preview),
                WsChannel::Events => continue,
            };

            if let Ok(json_value) = value {
                map.insert(channel.as_str().to_owned(), json_value);
            }
        }

        serde_json::Value::Object(map)
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct FramesConfig {
    pub(super) fps: u32,
    pub(super) format: FrameFormat,
    pub(super) zones: Vec<String>,
}

#[derive(Debug, Clone)]
pub(super) enum FrameZoneSelection {
    All,
    Named(HashSet<String>),
}

impl FrameZoneSelection {
    pub(super) fn new(selected: &[String]) -> Self {
        if selected.iter().any(|zone| zone == "all") {
            Self::All
        } else {
            Self::Named(selected.iter().cloned().collect())
        }
    }

    #[cfg(test)]
    pub(super) fn select<'a>(
        &self,
        zones: &'a [hypercolor_types::event::ZoneColors],
    ) -> Vec<&'a hypercolor_types::event::ZoneColors> {
        match self {
            Self::All => zones.iter().collect(),
            Self::Named(_) => zones
                .iter()
                .filter(|zone| self.includes(zone.zone_id.as_str()))
                .collect(),
        }
    }

    pub(super) fn includes(&self, zone_id: &str) -> bool {
        match self {
            Self::All => true,
            Self::Named(selected) => selected.contains(zone_id),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct ActiveFramesConfig {
    pub(super) config: FramesConfig,
    pub(super) selection_hash: u64,
    pub(super) selection: FrameZoneSelection,
}

impl ActiveFramesConfig {
    pub(super) fn new(config: FramesConfig) -> Self {
        let selection_hash = frame_selection_hash(&config.zones);
        let selection = FrameZoneSelection::new(&config.zones);
        Self {
            config,
            selection_hash,
            selection,
        }
    }
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

#[derive(Debug, Clone, Serialize)]
pub(super) struct SpectrumConfig {
    pub(super) fps: u32,
    pub(super) bins: u16,
}

impl Default for SpectrumConfig {
    fn default() -> Self {
        Self { fps: 30, bins: 64 }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct CanvasConfig {
    pub(super) fps: u32,
    pub(super) format: CanvasFormat,
    pub(super) width: u32,
    pub(super) height: u32,
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

#[derive(Debug, Clone, Serialize)]
pub(super) struct MetricsConfig {
    pub(super) interval_ms: u32,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self { interval_ms: 1000 }
    }
}

/// Configuration for the per-display preview channel. `device_id` is
/// `None` until the client sends its first subscribe with a target —
/// once set, the relay task follows that device's JPEG frame watch and
/// streams every new frame out as a binary `0x07` payload.
#[derive(Debug, Clone, Serialize)]
pub(super) struct DisplayPreviewConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) device_id: Option<String>,
    pub(super) fps: u32,
}

impl Default for DisplayPreviewConfig {
    fn default() -> Self {
        Self {
            device_id: None,
            fps: 15,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum FrameFormat {
    Binary,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CanvasFormat {
    Rgb,
    Rgba,
    Jpeg,
}

/// Client-to-server subscription messages.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ClientMessage {
    /// Subscribe to one or more channels.
    Subscribe {
        channels: Vec<String>,
        #[serde(default)]
        config: Option<ChannelConfigPatch>,
    },
    /// Unsubscribe from one or more channels.
    Unsubscribe { channels: Vec<String> },
    /// REST-equivalent command execution over WS.
    Command {
        id: String,
        method: String,
        path: String,
        #[serde(default)]
        body: Option<serde_json::Value>,
    },
}

#[derive(Debug, Deserialize, Default)]
pub(super) struct ChannelConfigPatch {
    #[serde(default)]
    pub(super) frames: Option<FramesConfigPatch>,
    #[serde(default)]
    pub(super) spectrum: Option<SpectrumConfigPatch>,
    #[serde(default)]
    pub(super) canvas: Option<CanvasConfigPatch>,
    #[serde(default)]
    pub(super) screen_canvas: Option<CanvasConfigPatch>,
    #[serde(default)]
    pub(super) web_viewport_canvas: Option<CanvasConfigPatch>,
    #[serde(default)]
    pub(super) metrics: Option<MetricsConfigPatch>,
    #[serde(default)]
    pub(super) display_preview: Option<DisplayPreviewConfigPatch>,
}

#[derive(Debug, Deserialize)]
pub(super) struct FramesConfigPatch {
    #[serde(default)]
    pub(super) fps: Option<u32>,
    #[serde(default)]
    pub(super) format: Option<FrameFormat>,
    #[serde(default)]
    pub(super) zones: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SpectrumConfigPatch {
    #[serde(default)]
    pub(super) fps: Option<u32>,
    #[serde(default)]
    pub(super) bins: Option<u16>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CanvasConfigPatch {
    #[serde(default)]
    pub(super) fps: Option<u32>,
    #[serde(default)]
    pub(super) format: Option<CanvasFormat>,
    #[serde(default)]
    pub(super) width: Option<u32>,
    #[serde(default)]
    pub(super) height: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MetricsConfigPatch {
    #[serde(default)]
    pub(super) interval_ms: Option<u32>,
}

/// Patch for `DisplayPreviewConfig`. `device_id` uses a double-Option so
/// clients can distinguish "leave as-is" (`device_id: undefined`) from
/// "clear the target" (`device_id: null`). Setting the outer `Some(None)`
/// detaches the relay and stops emitting frames.
///
/// The custom `deserialize_with` is required because plain
/// `Option<Option<String>>` with serde's default behavior collapses
/// `null` and missing-key to the same `None` — losing the tri-state we
/// need for "clear".
#[derive(Debug, Deserialize)]
pub(super) struct DisplayPreviewConfigPatch {
    #[serde(
        default,
        deserialize_with = "deserialize_double_option_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) device_id: Option<Option<String>>,
    #[serde(default)]
    pub(super) fps: Option<u32>,
}

/// Deserialize a double-Option so `null` maps to `Some(None)` (explicit
/// clear) and a missing key keeps the outer `None` (via `#[serde(default)]`).
/// Without this helper serde's default collapses both into `None`.
fn deserialize_double_option_string<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

/// Server-to-client acknowledgment messages.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ServerMessage {
    /// Initial hello with state snapshot.
    Hello {
        version: String,
        server: ServerIdentity,
        state: HelloState,
        capabilities: Vec<String>,
        subscriptions: Vec<String>,
    },
    /// Subscribe acknowledgment.
    Subscribed {
        channels: Vec<String>,
        config: serde_json::Value,
    },
    /// Unsubscribe acknowledgment.
    Unsubscribed {
        channels: Vec<String>,
        remaining: Vec<String>,
    },
    /// Event relay from the bus.
    Event {
        event: String,
        timestamp: String,
        data: serde_json::Value,
    },
    /// Periodic performance metrics snapshot.
    Metrics {
        timestamp: String,
        data: MetricsPayload,
    },
    /// Backpressure warning for dropped binary channel payloads.
    Backpressure {
        dropped_frames: u32,
        channel: String,
        recommendation: String,
        suggested_fps: u32,
    },
    /// Protocol-level request error.
    Error {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
    },
    /// Command response envelope for WS command execution.
    Response {
        id: String,
        status: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<serde_json::Value>,
    },
}

#[derive(Debug, Serialize)]
pub(super) struct HelloState {
    pub(super) running: bool,
    pub(super) paused: bool,
    pub(super) brightness: u8,
    pub(super) fps: HelloFps,
    pub(super) effect: Option<NameRef>,
    pub(super) scene: Option<SceneRef>,
    pub(super) profile: Option<NameRef>,
    pub(super) layout: Option<NameRef>,
    pub(super) device_count: usize,
    pub(super) total_leds: usize,
}

#[derive(Debug, Serialize)]
pub(super) struct HelloFps {
    pub(super) target: u32,
    pub(super) actual: f64,
}

#[derive(Debug, Serialize)]
pub(super) struct NameRef {
    pub(super) id: String,
    pub(super) name: String,
}

#[derive(Debug, Serialize)]
pub(super) struct SceneRef {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) snapshot_locked: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct MetricsPayload {
    pub(super) fps: MetricsFps,
    pub(super) frame_time: MetricsFrameTime,
    pub(super) stages: MetricsStages,
    pub(super) pacing: MetricsPacing,
    pub(super) timeline: MetricsTimeline,
    pub(super) render_surfaces: MetricsRenderSurfaces,
    pub(super) preview: MetricsPreview,
    pub(super) copies: MetricsCopies,
    pub(super) memory: MetricsMemory,
    pub(super) devices: MetricsDevices,
    pub(super) websocket: MetricsWebsocket,
}

#[derive(Debug, Serialize)]
pub(super) struct MetricsFps {
    pub(super) target: u32,
    pub(super) ceiling: u32,
    pub(super) actual: f64,
    pub(super) dropped: u32,
}

#[derive(Debug, Serialize)]
#[allow(
    clippy::struct_field_names,
    reason = "JSON keys mirror protocol field names from the WebSocket spec"
)]
pub(super) struct MetricsFrameTime {
    pub(super) avg_ms: f64,
    pub(super) p95_ms: f64,
    pub(super) p99_ms: f64,
    pub(super) max_ms: f64,
}

#[derive(Debug, Serialize)]
#[allow(
    clippy::struct_field_names,
    reason = "JSON keys mirror protocol field names from the WebSocket spec"
)]
pub(super) struct MetricsStages {
    pub(super) input_sampling_ms: f64,
    pub(super) producer_rendering_ms: f64,
    pub(super) composition_ms: f64,
    pub(super) effect_rendering_ms: f64,
    pub(super) spatial_sampling_ms: f64,
    pub(super) device_output_ms: f64,
    pub(super) preview_postprocess_ms: f64,
    pub(super) event_bus_ms: f64,
    pub(super) coordination_overhead_ms: f64,
}

#[derive(Debug, Serialize)]
#[allow(
    clippy::struct_field_names,
    reason = "JSON keys mirror protocol field names from the WebSocket spec"
)]
pub(super) struct MetricsPacing {
    pub(super) jitter_avg_ms: f64,
    pub(super) jitter_p95_ms: f64,
    pub(super) jitter_max_ms: f64,
    pub(super) wake_delay_avg_ms: f64,
    pub(super) wake_delay_p95_ms: f64,
    pub(super) wake_delay_max_ms: f64,
    pub(super) frame_age_ms: f64,
    pub(super) reused_inputs: u32,
    pub(super) reused_canvas: u32,
    pub(super) retained_effect: u32,
    pub(super) retained_screen: u32,
    pub(super) composition_bypassed: u32,
}

#[derive(Debug, Serialize)]
#[allow(
    clippy::struct_field_names,
    reason = "JSON keys mirror protocol field names from the WebSocket spec"
)]
pub(super) struct MetricsTimeline {
    pub(super) frame_token: u64,
    pub(super) compositor_backend: String,
    pub(super) gpu_zone_sampling: bool,
    pub(super) gpu_sample_deferred: bool,
    pub(super) gpu_sample_retry_hit: bool,
    pub(super) gpu_sample_wait_blocked: bool,
    pub(super) cpu_readback_skipped: bool,
    pub(super) budget_ms: f64,
    pub(super) wake_late_ms: f64,
    pub(super) logical_layer_count: u32,
    pub(super) render_group_count: u32,
    pub(super) scene_active: bool,
    pub(super) scene_transition_active: bool,
    pub(super) scene_snapshot_done_ms: f64,
    pub(super) input_done_ms: f64,
    pub(super) producer_done_ms: f64,
    pub(super) composition_done_ms: f64,
    pub(super) sampling_done_ms: f64,
    pub(super) output_done_ms: f64,
    pub(super) publish_done_ms: f64,
    pub(super) frame_done_ms: f64,
}

#[derive(Debug, Serialize)]
#[allow(
    clippy::struct_field_names,
    reason = "JSON keys mirror protocol field names from the WebSocket spec"
)]
pub(super) struct MetricsCopies {
    pub(super) full_frame_count: u32,
    pub(super) full_frame_kb: f64,
}

#[derive(Debug, Serialize)]
pub(super) struct MetricsRenderSurfaces {
    pub(super) slot_count: u32,
    pub(super) free_slots: u32,
    pub(super) published_slots: u32,
    pub(super) dequeued_slots: u32,
    pub(super) canvas_receivers: u32,
}

#[derive(Debug, Serialize)]
pub(super) struct MetricsPreview {
    pub(super) canvas_receivers: u32,
    pub(super) screen_canvas_receivers: u32,
    pub(super) web_viewport_canvas_receivers: u32,
    pub(super) canvas_frames_published: u64,
    pub(super) screen_canvas_frames_published: u64,
    pub(super) web_viewport_canvas_frames_published: u64,
    pub(super) latest_canvas_frame_number: u32,
    pub(super) latest_screen_canvas_frame_number: u32,
    pub(super) latest_web_viewport_canvas_frame_number: u32,
    pub(super) canvas_demand: MetricsPreviewDemand,
    pub(super) screen_canvas_demand: MetricsPreviewDemand,
    pub(super) web_viewport_canvas_demand: MetricsPreviewDemand,
}

#[derive(Debug, Serialize)]
pub(super) struct MetricsPreviewDemand {
    pub(super) subscribers: u32,
    pub(super) max_fps: u32,
    pub(super) max_width: u32,
    pub(super) max_height: u32,
    pub(super) any_full_resolution: bool,
    pub(super) any_rgb: bool,
    pub(super) any_rgba: bool,
    pub(super) any_jpeg: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct MetricsMemory {
    pub(super) daemon_rss_mb: f64,
    pub(super) servo_rss_mb: f64,
    pub(super) canvas_buffer_kb: u32,
}

#[derive(Debug, Serialize)]
pub(super) struct MetricsDevices {
    pub(super) connected: usize,
    pub(super) total_leds: usize,
    pub(super) output_errors: u32,
}

#[derive(Debug, Serialize)]
pub(super) struct MetricsWebsocket {
    pub(super) client_count: usize,
    pub(super) bytes_sent_per_sec: f64,
    pub(super) frame_payload_builds: u64,
    pub(super) frame_payload_cache_hits: u64,
    pub(super) canvas_payload_builds: u64,
    pub(super) canvas_payload_cache_hits: u64,
}

#[derive(Debug)]
pub(super) struct WsProtocolError {
    pub(super) code: &'static str,
    pub(super) message: String,
    pub(super) details: Option<serde_json::Value>,
}

impl WsProtocolError {
    pub(super) fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_request",
            message: message.into(),
            details: None,
        }
    }

    pub(super) fn invalid_config(field: &'static str, message: &'static str) -> Self {
        Self {
            code: "invalid_config",
            message: format!("Invalid configuration for {field}: {message}"),
            details: Some(json!({"field": field, "reason": message})),
        }
    }

    pub(super) fn unsupported_channel(channel: &str) -> Self {
        Self {
            code: "unsupported_channel",
            message: format!("Channel '{channel}' is not supported by this server"),
            details: Some(json!({"channel": channel})),
        }
    }

    pub(super) fn into_message(self) -> ServerMessage {
        ServerMessage::Error {
            code: self.code.to_owned(),
            message: self.message,
            details: self.details,
        }
    }
}

pub(super) fn frame_selection_hash(selected: &[String]) -> u64 {
    if selected.iter().any(|zone| zone == "all") {
        return 0;
    }

    let mut hasher = DefaultHasher::new();
    selected.len().hash(&mut hasher);
    for zone in selected {
        zone.hash(&mut hasher);
    }
    hasher.finish()
}

pub(super) fn validate_range(
    value: u32,
    min: u32,
    max: u32,
    field: &'static str,
    message: &'static str,
) -> Result<(), WsProtocolError> {
    if !(min..=max).contains(&value) {
        return Err(WsProtocolError::invalid_config(field, message));
    }
    Ok(())
}

pub(super) fn parse_channels(channels: &[String]) -> Result<Vec<WsChannel>, WsProtocolError> {
    if channels.is_empty() {
        return Err(WsProtocolError::invalid_request(
            "channels must contain at least one channel",
        ));
    }

    let mut parsed = Vec::with_capacity(channels.len());
    for channel in channels {
        let parsed_channel = WsChannel::parse(channel).ok_or_else(|| {
            WsProtocolError::invalid_request(format!("Unknown channel '{channel}'"))
        })?;

        if !parsed_channel.is_supported() {
            return Err(WsProtocolError::unsupported_channel(channel));
        }

        parsed.push(parsed_channel);
    }

    Ok(parsed)
}

pub(super) fn sorted_channel_names(channels: ChannelSet) -> Vec<String> {
    let mut names: Vec<String> = channels
        .iter()
        .map(|channel| channel.as_str().to_owned())
        .collect();
    names.sort();
    names
}

pub(super) fn unique_sorted_channel_names(channels: &[WsChannel]) -> Vec<String> {
    sorted_channel_names(ChannelSet::from_channels(channels))
}

pub(super) fn ws_capabilities() -> Vec<String> {
    let mut capabilities: Vec<String> = WsChannel::SUPPORTED
        .iter()
        .map(|channel| channel.as_str().to_owned())
        .collect();
    capabilities.push("commands".to_owned());
    capabilities.push("canvas_format_jpeg".to_owned());
    capabilities
}

pub(super) fn event_message_parts(
    event: &hypercolor_types::event::HypercolorEvent,
) -> (String, serde_json::Value) {
    let serialized = serde_json::to_value(event).ok();
    let event_type = serialized
        .as_ref()
        .and_then(|value| value.get("type"))
        .and_then(serde_json::Value::as_str);

    let event_name = if let Some(event_type) = event_type {
        to_snake_case(event_type)
    } else {
        format!("{:?}", event.category()).to_lowercase()
    };
    let event_data = serialized
        .and_then(|value| value.get("data").cloned())
        .unwrap_or_else(|| json!({}));

    (event_name, event_data)
}

pub(super) fn to_snake_case(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut previous_was_lower_or_digit = false;

    for ch in input.chars() {
        if ch.is_ascii_uppercase() {
            if previous_was_lower_or_digit {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            previous_was_lower_or_digit = false;
        } else {
            out.push(ch);
            previous_was_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        }
    }

    out
}

pub(super) fn should_relay_event(
    event: &hypercolor_types::event::HypercolorEvent,
    channels: ChannelSet,
) -> bool {
    if !channels.contains(WsChannel::Events) {
        return false;
    }

    if matches!(
        event,
        hypercolor_types::event::HypercolorEvent::FrameRendered { .. }
    ) && (channels.contains(WsChannel::Frames)
        || channels.contains(WsChannel::Canvas)
        || channels.contains(WsChannel::ScreenCanvas)
        || channels.contains(WsChannel::Metrics))
    {
        return false;
    }

    true
}
