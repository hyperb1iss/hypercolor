//! Event bus types — system-wide event taxonomy.
//!
//! Every discrete state change in Hypercolor flows through the event bus
//! as a [`HypercolorEvent`]. Events are categorized for subscription
//! filtering and prioritized for delivery guarantees.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::scene::{RenderGroupId, RenderGroupRole, SceneId};
use crate::session::SessionEvent;

// ── Supporting Types ────────────────────────────────────────────────────

/// Lightweight reference to an effect (avoids cloning full metadata into events).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectRef {
    pub id: String,
    pub name: String,
    /// `"wgpu"` | `"servo"`
    pub engine: String,
}

/// Lightweight reference to a discovered device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceRef {
    pub id: String,
    pub name: String,
    pub backend: String,
    pub led_count: u32,
}

/// Lightweight reference to a layout zone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZoneRef {
    pub zone_id: String,
    pub device_id: String,
    pub topology: String,
    pub led_count: u32,
}

/// Lightweight reference to a transition in progress.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionRef {
    /// `"crossfade"` | `"cut"` | `"dissolve"`
    pub transition_type: String,
    pub duration_ms: u32,
}

/// What triggered a state change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeTrigger {
    User,
    Profile,
    Scene,
    Api,
    Cli,
    Mcp,
    Dbus,
    Webhook,
    System,
}

/// Why a device was disconnected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisconnectReason {
    /// Device was removed by the user.
    Removed,
    /// Communication error (USB disconnect, network failure).
    Error,
    /// Heartbeat/keepalive timeout.
    Timeout,
    /// Daemon is shutting down.
    Shutdown,
    /// User explicitly disconnected.
    User,
}

/// Why an effect was stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectStopReason {
    /// Another effect was started (normal replacement).
    Replaced,
    /// User or API explicitly stopped the effect.
    Stopped,
    /// The effect crashed or failed to render.
    Error,
    /// Rendering was paused.
    Paused,
    /// Daemon is shutting down.
    Shutdown,
}

/// How a render group changed inside the active scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderGroupChangeKind {
    Created,
    Updated,
    Removed,
    ControlsPatched,
}

/// Why the active scene changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneChangeReason {
    UserActivate,
    UserDeactivate,
    EffectApplied,
    DaemonStart,
}

/// Error severity levels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warning,
    Error,
    Critical,
}

/// Press/release semantics for discrete host input events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputButtonState {
    Pressed,
    Released,
    Repeated,
}

/// MIDI transport-control messages that matter to rhythmic lighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiRealtimeMessage {
    Clock,
    Start,
    Continue,
    Stop,
}

/// A discrete host input event from keyboard or MIDI sources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputEvent {
    /// A host keyboard key changed state.
    Key {
        source_id: String,
        key: String,
        state: InputButtonState,
    },

    /// A MIDI note changed state.
    MidiNote {
        source_id: String,
        channel: u8,
        note: u8,
        velocity: u8,
        state: InputButtonState,
    },

    /// A MIDI control-change message was received.
    MidiControlChange {
        source_id: String,
        channel: u8,
        controller: u8,
        value: u8,
    },

    /// A MIDI pitch-bend message was received.
    MidiPitchBend {
        source_id: String,
        channel: u8,
        value: i16,
    },

    /// A MIDI realtime/transport message was received.
    MidiRealtime {
        source_id: String,
        message: MidiRealtimeMessage,
    },
}

/// Context dimensions for automation triggers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextType {
    /// Time-of-day bracket changed (e.g., "morning" -> "afternoon").
    TimeOfDay,
    /// Active application/window changed (desktop integration).
    ActiveWindow,
    /// System entered or exited idle state.
    IdleState,
    /// User presence changed (e.g., via Home Assistant).
    Presence,
    /// Custom context from webhook or external integration.
    Custom,
}

/// Lightweight control value for event payloads (3 variants).
///
/// Used within [`HypercolorEvent::EffectControlChanged`] to carry
/// old/new values across the event bus without pulling in the full
/// 7-variant [`effect::ControlValue`](crate::effect::ControlValue).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EventControlValue {
    Number(f32),
    Boolean(bool),
    String(String),
}

/// Per-stage frame timing in microseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameTiming {
    /// Time spent producing source surfaces before composition.
    #[serde(default)]
    pub producer_us: u32,
    /// Time spent latching and composing the frame set.
    #[serde(default)]
    pub composition_us: u32,
    /// Time to render the effect to the canvas.
    ///
    /// This remains the total Stage 2 cost for compatibility and equals
    /// `producer_us + composition_us` for new senders.
    pub render_us: u32,
    /// Time to sample LED positions from the canvas.
    pub sample_us: u32,
    /// Time to push frame data to all device backends.
    pub push_us: u32,
    /// Total frame time including overhead.
    pub total_us: u32,
    /// Frame time budget in microseconds (`1_000_000 / target_fps`).
    pub budget_us: u32,
}

// ── Frame Data Types ────────────────────────────────────────────────────

/// LED color data for all active zones in the current layout.
///
/// Published at render frame rate via `watch::Sender`. Subscribers skip
/// stale frames automatically — only the latest value matters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameData {
    /// Monotonically increasing frame counter.
    pub frame_number: u32,
    /// Millis since daemon start.
    pub timestamp_ms: u32,
    /// Per-zone color data, ordered consistently with the active layout.
    pub zones: Vec<ZoneColors>,
}

impl FrameData {
    /// Creates an empty frame with no zone data.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            frame_number: 0,
            timestamp_ms: 0,
            zones: Vec::new(),
        }
    }

    /// Creates a new frame with the given zone data.
    #[must_use]
    pub fn new(zones: Vec<ZoneColors>, frame_number: u32, timestamp_ms: u32) -> Self {
        Self {
            frame_number,
            timestamp_ms,
            zones,
        }
    }

    /// Total LED count across all zones.
    #[must_use]
    pub fn total_leds(&self) -> usize {
        self.zones.iter().map(|z| z.colors.len()).sum()
    }
}

/// Colors for a single device zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneColors {
    /// Stable zone identifier (e.g., `"wled_strip_1:zone_0"`).
    pub zone_id: String,
    /// RGB triplets, one per LED. Length matches the zone's LED count.
    pub colors: Vec<[u8; 3]>,
}

// ── Spectrum Data Types ─────────────────────────────────────────────────

/// Audio spectrum analysis data.
///
/// Published by the audio processor at ~30-60 fps via `watch::Sender`.
/// Subscribers skip stale data automatically -- only the latest value matters.
#[derive(Debug, Clone, PartialEq)]
pub struct SpectrumData {
    /// Millis since daemon start.
    pub timestamp_ms: u32,

    /// Overall audio level (RMS), 0.0-1.0.
    pub level: f32,
    /// Low-frequency energy, 0.0-1.0.
    pub bass: f32,
    /// Mid-frequency energy, 0.0-1.0.
    pub mid: f32,
    /// High-frequency energy, 0.0-1.0.
    pub treble: f32,

    /// Beat detected this frame.
    pub beat: bool,
    /// Beat detection confidence, 0.0-1.0.
    pub beat_confidence: f32,
    /// Estimated BPM (`None` if insufficient data).
    pub bpm: Option<f32>,

    /// FFT frequency bins, normalized 0.0-1.0.
    /// Full resolution: 200 bins.
    pub bins: Vec<f32>,
}

impl SpectrumData {
    /// Returns an empty spectrum with all values zeroed out.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            timestamp_ms: 0,
            level: 0.0,
            bass: 0.0,
            mid: 0.0,
            treble: 0.0,
            beat: false,
            beat_confidence: 0.0,
            bpm: None,
            bins: Vec::new(),
        }
    }

    /// Downsample bins to the requested count using averaging.
    ///
    /// Returns empty if bins are empty or `target_bins` is zero.
    /// Returns a clone if `target_bins >= self.bins.len()`.
    #[must_use]
    pub fn downsample(&self, target_bins: usize) -> Vec<f32> {
        if self.bins.is_empty() || target_bins == 0 {
            return Vec::new();
        }
        if target_bins >= self.bins.len() {
            return self.bins.clone();
        }
        let bin_count = self.bins.len();
        (0..target_bins)
            .map(|i| {
                // Integer arithmetic for chunk boundaries avoids f32->usize casts.
                let start = i * bin_count / target_bins;
                let end = ((i + 1) * bin_count / target_bins).min(bin_count);
                let slice = &self.bins[start..end];
                let sum: f32 = slice.iter().sum();
                // Slice is never empty: target_bins < bin_count guarantees >= 1 element per chunk.
                #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
                let avg = sum / slice.len() as f32;
                avg
            })
            .collect()
    }
}

// ── Event Taxonomy ──────────────────────────────────────────────────────

/// Every discrete state change in Hypercolor.
///
/// Serialized as externally tagged: `{ "type": "EffectStarted", "data": { ... } }`.
/// The `timestamp` field is added by the bus infrastructure, not by the event producer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum HypercolorEvent {
    // ── Device Events ───────────────────────────────────────────────
    /// A device was found during discovery but not yet connected.
    DeviceDiscovered {
        device_id: String,
        name: String,
        backend: String,
        led_count: u32,
        /// Network address, USB path, or other locator.
        address: Option<String>,
    },

    /// A device backend successfully connected and the device is ready
    /// to receive frame data.
    DeviceConnected {
        device_id: String,
        name: String,
        backend: String,
        led_count: u32,
        zones: Vec<ZoneRef>,
    },

    /// A device was disconnected (removed, timed out, or errored).
    DeviceDisconnected {
        device_id: String,
        reason: DisconnectReason,
        /// Whether the daemon will attempt automatic reconnection.
        will_retry: bool,
    },

    /// A device backend encountered a communication error.
    DeviceError {
        device_id: String,
        error: String,
        /// Whether the daemon will retry automatically.
        recoverable: bool,
    },

    /// Firmware or hardware metadata read from a device.
    DeviceFirmwareInfo {
        device_id: String,
        firmware_version: Option<String>,
        hardware_version: Option<String>,
        manufacturer: Option<String>,
        model: Option<String>,
        /// Freeform key-value metadata (MAC address, chip type, etc.).
        extra: HashMap<String, String>,
    },

    /// A device backend reported a state change (zone reconfiguration,
    /// LED count change, etc.).
    DeviceStateChanged {
        device_id: String,
        changes: HashMap<String, serde_json::Value>,
    },

    /// Device discovery scan started.
    DeviceDiscoveryStarted {
        /// Which backends are being scanned.
        backends: Vec<String>,
    },

    /// Device discovery scan completed for all requested backends.
    DeviceDiscoveryCompleted {
        found: Vec<DeviceRef>,
        duration_ms: u64,
    },

    // ── Effect Events ───────────────────────────────────────────────
    /// A new effect has been loaded and rendering has begun.
    EffectStarted {
        effect: EffectRef,
        /// What caused the start: user selection, profile load, scene trigger, etc.
        trigger: ChangeTrigger,
        /// If this replaced a previous effect, reference it here.
        previous: Option<EffectRef>,
        /// Transition type applied (if any).
        transition: Option<TransitionRef>,
    },

    /// The active effect has been stopped.
    EffectStopped {
        effect: EffectRef,
        reason: EffectStopReason,
    },

    /// A control value on the active effect was updated.
    EffectControlChanged {
        effect_id: String,
        control_id: String,
        old_value: EventControlValue,
        new_value: EventControlValue,
        trigger: ChangeTrigger,
    },

    /// A compositing layer was added to the effect stack.
    EffectLayerAdded {
        layer_id: String,
        effect: EffectRef,
        /// Stack index (0 = bottom).
        index: u32,
        blend_mode: String,
        opacity: f32,
    },

    /// A compositing layer was removed from the effect stack.
    EffectLayerRemoved { layer_id: String, effect_id: String },

    /// The effect registry was rescanned (hot-reload or manual trigger).
    EffectRegistryUpdated {
        /// Number of newly discovered effects.
        added: usize,
        /// Number of effects removed (source file deleted).
        removed: usize,
        /// Number of effects re-loaded (source file modified).
        updated: usize,
    },

    /// An effect failed to load, render, or initialize.
    EffectError {
        effect_id: String,
        error: String,
        /// Whether the engine fell back to a previous or safe default effect.
        fallback: Option<String>,
    },

    // ── Scene Events ────────────────────────────────────────────────
    /// A scene was triggered and its associated profile is being applied.
    SceneActivated {
        scene_id: String,
        scene_name: String,
        /// `"schedule"` | `"webhook"` | `"event"` | `"device"` | `"input"` | `"manual"`
        trigger_type: String,
        profile_id: String,
    },

    /// A scene transition has begun (crossfade in progress).
    SceneTransitionStarted {
        scene_id: String,
        from_profile: Option<String>,
        to_profile: String,
        duration_ms: u32,
    },

    /// A scene transition completed (new profile fully active).
    SceneTransitionComplete {
        scene_id: String,
        profile_id: String,
    },

    /// A scene was enabled or disabled.
    SceneEnabled { scene_id: String, enabled: bool },

    /// A render group in a scene changed.
    RenderGroupChanged {
        scene_id: SceneId,
        group_id: RenderGroupId,
        role: RenderGroupRole,
        kind: RenderGroupChangeKind,
    },

    /// The active scene changed.
    ActiveSceneChanged {
        previous: Option<SceneId>,
        current: SceneId,
        reason: SceneChangeReason,
    },

    // ── Audio Events ────────────────────────────────────────────────
    /// The audio input source changed.
    AudioSourceChanged {
        /// Previous source name, `None` if first activation.
        previous: Option<String>,
        current: String,
        sample_rate: u32,
    },

    /// Beat detected in the audio stream.
    BeatDetected {
        /// Confidence in this onset. 0.0–1.0.
        confidence: f32,
        /// Current estimated BPM (`None` if insufficient data).
        bpm: Option<f32>,
        /// Phase within the current beat cycle. 0.0–1.0.
        phase: f32,
    },

    /// Periodic audio level summary (default: 10 Hz).
    AudioLevelUpdate {
        /// Overall audio level (RMS), 0.0–1.0.
        level: f32,
        bass: f32,
        mid: f32,
        treble: f32,
        /// Whether a beat was detected in this analysis window.
        beat: bool,
    },

    /// Audio capture started.
    AudioStarted {
        source_name: String,
        sample_rate: u32,
    },

    /// Audio capture stopped.
    AudioStopped { reason: String },

    /// Screen capture started.
    CaptureStarted {
        source_name: String,
        resolution: (u32, u32),
    },

    /// Screen capture stopped.
    CaptureStopped { reason: String },

    /// A discrete host input event was observed.
    InputEventReceived { event: InputEvent },

    // ── System Events ───────────────────────────────────────────────
    /// A frame was rendered and pushed to all device backends.
    FrameRendered {
        frame_number: u32,
        timing: FrameTiming,
    },

    /// The measured or target FPS changed.
    FpsChanged {
        old_target: u32,
        new_target: u32,
        measured: f32,
    },

    /// A profile was applied (all its settings are now active).
    ProfileLoaded {
        profile_id: String,
        profile_name: String,
        trigger: ChangeTrigger,
    },

    /// A profile was saved (created or updated).
    ProfileSaved {
        profile_id: String,
        profile_name: String,
        is_new: bool,
    },

    /// A profile was deleted.
    ProfileDeleted { profile_id: String },

    /// A configuration value changed (daemon config, not effect controls).
    ConfigChanged {
        /// Dotted path to the changed key (e.g., `"daemon.fps"`, `"audio.gain"`).
        key: String,
        old_value: Option<serde_json::Value>,
        new_value: serde_json::Value,
    },

    /// A graceful shutdown has been requested.
    ShutdownRequested {
        /// `"signal"` | `"user"` | `"api"` | `"dbus"`
        source: String,
        /// Seconds until shutdown. 0 = immediate.
        grace_period_secs: u32,
    },

    /// Daemon has finished startup and is ready to accept commands.
    DaemonStarted {
        version: String,
        pid: u32,
        device_count: u32,
        effect_count: u32,
    },

    /// Daemon shutdown is imminent — last event before the bus closes.
    DaemonShutdown {
        /// `"signal"` | `"user"` | `"error"` | `"restart"`
        reason: String,
    },

    /// Global brightness changed.
    BrightnessChanged { old: u8, new_value: u8 },

    /// Rendering paused (all LEDs go dark).
    Paused,

    /// Rendering resumed.
    Resumed,

    /// Session/power-awareness state changed.
    SessionChanged(SessionEvent),

    /// A system-level error occurred.
    Error {
        code: String,
        message: String,
        severity: Severity,
    },

    // ── Automation Events ───────────────────────────────────────────
    /// A scene trigger condition was met and the trigger fired.
    TriggerFired {
        trigger_id: String,
        scene_id: String,
        /// `"schedule"` | `"webhook"` | `"event"` | `"device"` | `"input"`
        trigger_type: String,
        /// Raw trigger payload (cron match time, webhook body, etc.).
        payload: serde_json::Value,
    },

    /// A time-based schedule activated (cron or solar trigger).
    ScheduleActivated {
        scene_id: String,
        scene_name: String,
        /// The cron expression or solar event that matched.
        schedule_expr: String,
        /// The profile that will be applied.
        profile_id: String,
    },

    /// Environmental or application context changed, potentially
    /// triggering scene re-evaluation.
    ContextChanged {
        /// The context dimension that changed.
        context_type: ContextType,
        /// Previous value (for debugging).
        previous: Option<String>,
        /// Current value.
        current: String,
    },

    // ── Layout Events ───────────────────────────────────────────────
    /// The active spatial layout changed.
    LayoutChanged {
        previous: Option<String>,
        current: String,
    },

    /// A zone was added to the active layout.
    LayoutZoneAdded { layout_id: String, zone: ZoneRef },

    /// A zone was removed from the active layout.
    LayoutZoneRemoved {
        layout_id: String,
        zone_id: String,
        device_id: String,
    },

    /// The active layout was modified (zone positions, sizes, or topology).
    LayoutUpdated { layout_id: String },

    // ── Integration Events ──────────────────────────────────────────
    /// A webhook was received from an external integration.
    WebhookReceived { webhook_id: String, source: String },

    /// An input source was added, removed, or reconfigured.
    InputSourceChanged {
        input_id: String,
        input_type: String,
        enabled: bool,
    },
}

// ── Event Categories ────────────────────────────────────────────────────

/// Event categories for subscription filtering.
///
/// Every event belongs to exactly one category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventCategory {
    Device,
    Effect,
    Scene,
    Audio,
    System,
    Automation,
    Layout,
    Input,
    Integration,
}

/// Delivery priority for events.
///
/// Higher-priority events receive stronger delivery guarantees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

// ── HypercolorEvent Methods ─────────────────────────────────────────────

impl HypercolorEvent {
    /// Returns the category this event belongs to, for filtering purposes.
    #[must_use]
    pub fn category(&self) -> EventCategory {
        match self {
            Self::DeviceDiscovered { .. }
            | Self::DeviceConnected { .. }
            | Self::DeviceDisconnected { .. }
            | Self::DeviceError { .. }
            | Self::DeviceFirmwareInfo { .. }
            | Self::DeviceStateChanged { .. }
            | Self::DeviceDiscoveryStarted { .. }
            | Self::DeviceDiscoveryCompleted { .. } => EventCategory::Device,

            Self::EffectStarted { .. }
            | Self::EffectStopped { .. }
            | Self::EffectControlChanged { .. }
            | Self::EffectLayerAdded { .. }
            | Self::EffectLayerRemoved { .. }
            | Self::EffectRegistryUpdated { .. }
            | Self::EffectError { .. } => EventCategory::Effect,

            Self::SceneActivated { .. }
            | Self::SceneTransitionStarted { .. }
            | Self::SceneTransitionComplete { .. }
            | Self::SceneEnabled { .. }
            | Self::RenderGroupChanged { .. }
            | Self::ActiveSceneChanged { .. } => EventCategory::Scene,

            Self::AudioSourceChanged { .. }
            | Self::BeatDetected { .. }
            | Self::AudioLevelUpdate { .. }
            | Self::AudioStarted { .. }
            | Self::AudioStopped { .. } => EventCategory::Audio,

            Self::FrameRendered { .. }
            | Self::FpsChanged { .. }
            | Self::ProfileLoaded { .. }
            | Self::ProfileSaved { .. }
            | Self::ProfileDeleted { .. }
            | Self::ConfigChanged { .. }
            | Self::ShutdownRequested { .. }
            | Self::DaemonStarted { .. }
            | Self::DaemonShutdown { .. }
            | Self::BrightnessChanged { .. }
            | Self::Paused
            | Self::Resumed
            | Self::SessionChanged(..)
            | Self::Error { .. } => EventCategory::System,

            Self::TriggerFired { .. }
            | Self::ScheduleActivated { .. }
            | Self::ContextChanged { .. } => EventCategory::Automation,

            Self::LayoutChanged { .. }
            | Self::LayoutZoneAdded { .. }
            | Self::LayoutZoneRemoved { .. }
            | Self::LayoutUpdated { .. } => EventCategory::Layout,

            Self::CaptureStarted { .. }
            | Self::CaptureStopped { .. }
            | Self::InputEventReceived { .. }
            | Self::InputSourceChanged { .. } => EventCategory::Input,

            Self::WebhookReceived { .. } => EventCategory::Integration,
        }
    }

    /// Returns this event's delivery priority.
    ///
    /// - **Critical:** Guaranteed delivery, never dropped.
    /// - **High:** Delivered within 1 ms of occurrence.
    /// - **Normal:** Delivered within 5 ms.
    /// - **Low:** Best-effort, may be coalesced or dropped under congestion.
    #[must_use]
    pub fn priority(&self) -> EventPriority {
        match self {
            Self::DaemonShutdown { .. }
            | Self::ShutdownRequested { .. }
            | Self::Error {
                severity: Severity::Critical,
                ..
            } => EventPriority::Critical,

            Self::DeviceConnected { .. }
            | Self::DeviceDisconnected { .. }
            | Self::DeviceError { .. } => EventPriority::High,

            Self::BeatDetected { .. }
            | Self::AudioLevelUpdate { .. }
            | Self::FrameRendered { .. }
            | Self::InputEventReceived { .. }
            | Self::DeviceDiscoveryCompleted { .. }
            | Self::LayoutUpdated { .. }
            | Self::WebhookReceived { .. } => EventPriority::Low,

            _ => EventPriority::Normal,
        }
    }
}
