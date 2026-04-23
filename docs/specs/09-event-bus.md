# 09 -- Event Bus & IPC Protocol

> The nervous system. Every signal between daemon, TUI, CLI, and web UI flows through here.

*Synthesized from: [ARCHITECTURE.md](../../ARCHITECTURE.md) (Event Bus, Render Loop), [05-api-design.md](../design/05-api-design.md) (Sections 3, 6, 8), [10-tui-cli.md](../design/10-tui-cli.md) (IPC Appendix B, TUI Performance).*

---

## Table of Contents

1. [Overview](#1-overview)
2. [HypercolorEvent Enum](#2-hypercolorevent-enum)
3. [Broadcast Channel](#3-broadcast-channel)
4. [Watch Channels](#4-watch-channels)
5. [FrameData Distribution](#5-framedata-distribution)
6. [SpectrumData Distribution](#6-spectrumdata-distribution)
7. [Subscription Model](#7-subscription-model)
8. [IPC Protocol for TUI/CLI](#8-ipc-protocol-for-tuicli)
9. [WebSocket Bridge](#9-websocket-bridge)
10. [Event Serialization](#10-event-serialization)
11. [Backpressure](#11-backpressure)
12. [Thread Safety](#12-thread-safety)

---

## 1. Overview

Hypercolor is a multi-process, multi-frontend architecture. One daemon renders effects and drives hardware. Multiple clients -- TUI, CLI, web UI, MCP, D-Bus consumers -- observe state and issue commands. The event bus is the connective tissue that makes all of this feel instantaneous and coherent.

Three distinct communication patterns coexist:

| Pattern | Channel Type | Semantics | Consumers |
|---------|-------------|-----------|-----------|
| **Events** | `broadcast::Sender` | Every subscriber sees every event. Ordered, fan-out. | All frontends, integrations |
| **Frame data** | `watch::Sender` | Latest-value only. Subscribers skip stale frames. | TUI LED preview, web UI canvas |
| **Spectrum data** | `watch::Sender` | Latest-value only. Subscribers skip stale data. | TUI spectrum widget, web UI visualizer |
| **Active effect** | `watch::Sender` | Latest active effect info. | TUI status bar, web UI header |
| **Current FPS** | `watch::Sender` | Latest measured FPS. | TUI/web performance display |

Events carry discrete state transitions (device connected, effect changed, error). Watch channels carry continuous high-frequency streams where only the most recent value matters. This dual-channel design means a slow WebSocket client never causes frame backpressure on the render loop -- it simply skips to the latest frame.

**Data flow topology:**

```
                     +---------------------------+
                     |       Render Loop          |
                     |   (60fps effect engine)    |
                     +----+---------+--------+----+
                          |         |        |
                 events   |  frame  | spectrum|  watch: fps, effect
                          |         |        |
                     +----v---------v--------v----+
                     |        HypercolorBus       |
                     |                             |
                     |  broadcast<Event>           |
                     |  watch<FrameData>           |
                     |  watch<SpectrumData>        |
                     |  watch<ActiveEffectInfo>    |
                     |  watch<FpsSnapshot>         |
                     +---+------+------+------+---+
                         |      |      |      |
               +---------v+ +--v----+ |  +---v----------+
               | WebSocket| | Unix  | |  | D-Bus        |
               | (Axum)   | |Socket | |  | (zbus)       |
               +----+-----+ +--+---+  |  +---+----------+
                    |           |      |      |
               +----v----+ +--v--+ +--v--+   |
               | Web UI  | | TUI | | CLI |   |
               |SvelteKit| |     | |     |   |
               +---------+ +-----+ +-----+   |
                                        +-----v---------+
                                        | Desktop / HA  |
                                        +---------------+
```

---

## 2. HypercolorEvent Enum

The complete taxonomy of every discrete state change in the system. All API surfaces -- WebSocket, Unix socket, D-Bus signals, MQTT -- deliver the same events with the same structure.

### 2.1 Rust Definition

```rust
use serde::{Serialize, Deserialize};
use std::collections::HashMap;

/// Every discrete state change in Hypercolor.
///
/// Serialized as externally tagged: `{ "type": "EffectStarted", "data": { ... } }`.
/// The `timestamp` field is added by the bus infrastructure, not by the event producer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum HypercolorEvent {
    // ── Device Events ─────────────────────────────────────────────

    /// A device was found during discovery but not yet connected.
    /// Fires once per newly-seen device per scan.
    DeviceDiscovered {
        device_id: String,
        name: String,
        backend: String,
        led_count: u32,
        /// Network address, USB path, or other locator.
        address: Option<String>,
    },

    /// A device backend successfully connected to a device and it is
    /// ready to receive frame data.
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

    /// A device backend encountered an error communicating with a device.
    DeviceError {
        device_id: String,
        error: String,
        /// Whether the daemon will retry automatically.
        recoverable: bool,
    },

    /// Firmware or hardware metadata was read from a device (after connect
    /// or after explicit query). Useful for diagnostics and device manager UI.
    DeviceFirmwareInfo {
        device_id: String,
        firmware_version: Option<String>,
        hardware_version: Option<String>,
        manufacturer: Option<String>,
        model: Option<String>,
        /// Freeform key-value metadata (e.g., MAC address, chip type).
        extra: HashMap<String, String>,
    },

    /// A device backend reported a state change (e.g., zone reconfiguration,
    /// LED count change after firmware update).
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

    // ── Effect Events ─────────────────────────────────────────────

    /// A new effect has been loaded and rendering has begun.
    /// Fires after the effect engine successfully initializes the effect.
    EffectStarted {
        effect: EffectRef,
        /// What caused the start: user selection, profile load, scene trigger, etc.
        trigger: ChangeTrigger,
        /// If this replaced a previous effect, reference it here.
        previous: Option<EffectRef>,
        /// Transition type applied (if any).
        transition: Option<TransitionRef>,
    },

    /// The active effect has been stopped. The canvas is now idle or
    /// showing a fallback (e.g., solid black). Fires on explicit stop,
    /// daemon pause, or effect unload.
    EffectStopped {
        effect: EffectRef,
        reason: EffectStopReason,
    },

    /// A control value on the active effect was updated.
    EffectControlChanged {
        effect_id: String,
        control_id: String,
        old_value: ControlValue,
        new_value: ControlValue,
        trigger: ChangeTrigger,
    },

    /// A compositing layer was added to the effect stack.
    /// (Phase 2+ -- multi-layer effect composition.)
    EffectLayerAdded {
        layer_id: String,
        effect: EffectRef,
        /// Stack index (0 = bottom).
        index: u32,
        blend_mode: String,
        opacity: f32,
    },

    /// A compositing layer was removed from the effect stack.
    EffectLayerRemoved {
        layer_id: String,
        effect_id: String,
    },

    /// An effect failed to load, render, or initialize.
    EffectError {
        effect_id: String,
        error: String,
        /// Whether the engine fell back to the previous effect or went to a safe default.
        fallback: Option<String>,
    },

    // ── Scene Events ──────────────────────────────────────────────

    /// A scene was triggered and its associated profile is being applied.
    SceneActivated {
        scene_id: String,
        scene_name: String,
        /// "schedule" | "webhook" | "event" | "device" | "input" | "manual"
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

    /// A scene transition completed (crossfade finished, new profile fully active).
    SceneTransitionComplete {
        scene_id: String,
        profile_id: String,
    },

    /// A scene was enabled or disabled.
    SceneEnabled {
        scene_id: String,
        enabled: bool,
    },

    // ── Audio Events ──────────────────────────────────────────────

    /// The audio input source changed (different PipeWire source, device
    /// swap, or audio subsystem reconfiguration).
    AudioSourceChanged {
        /// Previous source name, None if this is the first activation.
        previous: Option<String>,
        current: String,
        sample_rate: u32,
    },

    /// Beat detected in the audio stream. High-frequency event, best-effort delivery.
    /// The audio processor fires this on every detected beat onset.
    BeatDetected {
        /// How confident the beat detector is in this onset. 0.0 - 1.0.
        confidence: f32,
        /// Current estimated BPM (None if insufficient data for estimation).
        bpm: Option<f32>,
        /// Phase within the current beat cycle. 0.0 - 1.0.
        phase: f32,
    },

    /// Periodic audio level summary. Fired at a fixed rate (default: 10Hz)
    /// to give subscribers a digestible audio state without requiring
    /// the full-resolution spectrum watch channel.
    AudioLevelUpdate {
        /// Overall audio level (RMS), 0.0 - 1.0.
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
    AudioStopped {
        reason: String,
    },

    /// Screen capture started.
    CaptureStarted {
        source_name: String,
        resolution: (u32, u32),
    },

    /// Screen capture stopped.
    CaptureStopped {
        reason: String,
    },

    // ── System Events ─────────────────────────────────────────────

    /// A frame was rendered and pushed to all device backends.
    /// Low-priority, primarily for performance monitoring and the debug view.
    FrameRendered {
        frame_number: u32,
        /// Per-stage timing in microseconds.
        timing: FrameTiming,
    },

    /// The measured or target FPS changed. Fires when the user changes the
    /// target FPS setting, or when the measured FPS deviates significantly
    /// (>5%) from the previous reported value.
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
    ProfileDeleted {
        profile_id: String,
    },

    /// A configuration value changed (daemon config, not effect controls).
    /// Covers any change to the TOML configuration: audio settings, network
    /// bindings, canvas resolution, etc.
    ConfigChanged {
        /// Dotted path to the changed key (e.g., "daemon.fps", "audio.gain").
        key: String,
        old_value: Option<serde_json::Value>,
        new_value: serde_json::Value,
    },

    /// A graceful shutdown has been requested. This is NOT the final event --
    /// `DaemonShutdown` fires after all cleanup. This event gives subscribers
    /// a chance to persist state before the bus closes.
    ShutdownRequested {
        /// "signal" | "user" | "api" | "dbus"
        source: String,
        /// Seconds until the daemon will shut down. 0 = immediate.
        grace_period_secs: u32,
    },

    /// Daemon has finished startup and is ready to accept commands.
    DaemonStarted {
        version: String,
        pid: u32,
        device_count: u32,
        effect_count: u32,
    },

    /// Daemon shutdown is imminent. This is the last event before the bus closes.
    /// Critical priority -- guaranteed delivery.
    DaemonShutdown {
        /// "signal" | "user" | "error" | "restart"
        reason: String,
    },

    /// Global brightness changed.
    BrightnessChanged {
        old: u8,
        new_value: u8,
    },

    /// Rendering paused (all LEDs go dark).
    Paused,

    /// Rendering resumed.
    Resumed,

    /// A system-level error occurred.
    Error {
        code: String,
        message: String,
        severity: Severity,
    },

    // ── Automation Events ─────────────────────────────────────────

    /// A scene trigger condition was met and the trigger fired.
    /// This precedes `SceneActivated` and carries the raw trigger data.
    TriggerFired {
        trigger_id: String,
        scene_id: String,
        /// The trigger type: "schedule", "webhook", "event", "device", "input".
        trigger_type: String,
        /// Raw trigger payload (cron match time, webhook body, etc.).
        payload: serde_json::Value,
    },

    /// A time-based schedule activated. Fires for cron and solar triggers.
    ScheduleActivated {
        scene_id: String,
        scene_name: String,
        /// The cron expression or solar event that matched.
        schedule_expr: String,
        /// The profile that will be applied.
        profile_id: String,
    },

    /// The environmental or application context changed, potentially
    /// triggering scene re-evaluation. Examples: time-of-day bracket
    /// change, active window changed, system idle state changed.
    ContextChanged {
        /// The context dimension that changed.
        context_type: ContextType,
        /// Previous value (for debugging).
        previous: Option<String>,
        /// Current value.
        current: String,
    },

    // ── Layout Events ─────────────────────────────────────────────

    /// The active spatial layout changed (different layout selected).
    LayoutChanged {
        previous: Option<String>,
        current: String,
    },

    /// A zone was added to the active layout.
    LayoutZoneAdded {
        layout_id: String,
        zone: ZoneRef,
    },

    /// A zone was removed from the active layout.
    LayoutZoneRemoved {
        layout_id: String,
        zone_id: String,
        device_id: String,
    },

    /// The active layout was modified (zone positions, sizes, or topology changed).
    LayoutUpdated {
        layout_id: String,
    },

    // ── Integration Events ────────────────────────────────────────

    /// A webhook was received from an external integration.
    WebhookReceived {
        webhook_id: String,
        source: String,
    },

    /// An input source was added, removed, or reconfigured.
    InputSourceChanged {
        input_id: String,
        input_type: String,
        enabled: bool,
    },
}
```

### 2.2 Supporting Types

```rust
/// Lightweight reference to an effect (avoids cloning full metadata into events).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectRef {
    pub id: String,
    pub name: String,
    /// "wgpu" | "servo"
    pub engine: String,
}

/// Lightweight reference to a discovered device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceRef {
    pub id: String,
    pub name: String,
    pub backend: String,
    pub led_count: u32,
}

/// Lightweight reference to a layout zone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneRef {
    pub zone_id: String,
    pub device_id: String,
    pub topology: String,
    pub led_count: u32,
}

/// Lightweight reference to a transition in progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionRef {
    /// "crossfade" | "cut" | "dissolve"
    pub transition_type: String,
    pub duration_ms: u32,
}

/// What triggered a state change.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Error severity levels.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warning,
    Error,
    Critical,
}

/// Context dimensions for automation triggers.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Per-stage frame timing in microseconds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameTiming {
    /// Time to render the effect to the canvas.
    pub render_us: u32,
    /// Time to sample LED positions from the canvas.
    pub sample_us: u32,
    /// Time to push frame data to all device backends.
    pub push_us: u32,
    /// Total frame time including overhead.
    pub total_us: u32,
    /// Frame time budget in microseconds (1_000_000 / target_fps).
    pub budget_us: u32,
}

/// Control values (effect parameters).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ControlValue {
    Number(f32),
    Boolean(bool),
    String(String),
}
```

### 2.3 Event Categories

Events are grouped into categories for filtering. Every event belongs to exactly one category.

| Category | Events | Use Cases |
|----------|--------|-----------|
| `device` | `DeviceDiscovered`, `DeviceConnected`, `DeviceDisconnected`, `DeviceError`, `DeviceFirmwareInfo`, `DeviceStateChanged`, `DeviceDiscoveryStarted`, `DeviceDiscoveryCompleted` | Device manager, health monitoring |
| `effect` | `EffectStarted`, `EffectStopped`, `EffectControlChanged`, `EffectLayerAdded`, `EffectLayerRemoved`, `EffectError` | UI state sync, effect browser |
| `scene` | `SceneActivated`, `SceneTransitionStarted`, `SceneTransitionComplete`, `SceneEnabled` | Automation UI, transition coordination |
| `audio` | `AudioSourceChanged`, `BeatDetected`, `AudioLevelUpdate`, `AudioStarted`, `AudioStopped` | Audio UI, reactive effect tuning |
| `system` | `FrameRendered`, `FpsChanged`, `ProfileLoaded`, `ProfileSaved`, `ProfileDeleted`, `ConfigChanged`, `ShutdownRequested`, `DaemonStarted`, `DaemonShutdown`, `BrightnessChanged`, `Paused`, `Resumed`, `Error` | System health, monitoring, debug view |
| `automation` | `TriggerFired`, `ScheduleActivated`, `ContextChanged` | Scene scheduler, context engine |
| `layout` | `LayoutChanged`, `LayoutZoneAdded`, `LayoutZoneRemoved`, `LayoutUpdated` | Spatial editor sync |
| `input` | `CaptureStarted`, `CaptureStopped`, `InputSourceChanged` | Input status UI |
| `integration` | `WebhookReceived` | External integration monitoring |

**Category derivation (compile-time):**

```rust
impl HypercolorEvent {
    /// Returns the category for filtering purposes.
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
            | Self::EffectError { .. } => EventCategory::Effect,

            Self::SceneActivated { .. }
            | Self::SceneTransitionStarted { .. }
            | Self::SceneTransitionComplete { .. }
            | Self::SceneEnabled { .. } => EventCategory::Scene,

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
            | Self::InputSourceChanged { .. } => EventCategory::Input,

            Self::WebhookReceived { .. } => EventCategory::Integration,
        }
    }

    /// Returns the event's delivery priority.
    pub fn priority(&self) -> EventPriority {
        match self {
            Self::DaemonShutdown { .. } => EventPriority::Critical,
            Self::Error { severity: Severity::Critical, .. } => EventPriority::Critical,
            Self::ShutdownRequested { .. } => EventPriority::Critical,

            Self::DeviceConnected { .. }
            | Self::DeviceDisconnected { .. }
            | Self::DeviceError { .. } => EventPriority::High,

            Self::BeatDetected { .. }
            | Self::AudioLevelUpdate { .. }
            | Self::FrameRendered { .. }
            | Self::DeviceDiscoveryCompleted { .. }
            | Self::LayoutUpdated { .. }
            | Self::WebhookReceived { .. } => EventPriority::Low,

            _ => EventPriority::Normal,
        }
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EventPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}
```

### 2.4 Event Priority & Delivery Guarantees

| Priority | Events | Delivery |
|----------|--------|----------|
| **Critical** | `DaemonShutdown`, `ShutdownRequested`, `Error(severity=Critical)` | Guaranteed delivery. Sent before bus teardown. Never dropped, even under backpressure. |
| **High** | `DeviceConnected`, `DeviceDisconnected`, `DeviceError` | Delivered within 1ms of occurrence. |
| **Normal** | `EffectStarted`, `EffectStopped`, `ProfileLoaded`, `BrightnessChanged`, `LayoutChanged`, `SceneActivated`, `ConfigChanged` | Delivered within 5ms. |
| **Low** | `BeatDetected`, `AudioLevelUpdate`, `FrameRendered`, `DeviceDiscoveryCompleted`, `LayoutUpdated`, `WebhookReceived` | Best-effort. May be coalesced or dropped if bus is congested. |

The `tokio::broadcast` channel has a buffer capacity of **256 events**. If a subscriber falls behind (e.g., a slow WebSocket client), it receives `tokio::sync::broadcast::error::RecvError::Lagged(n)` and must request a state snapshot to recover. This is by design -- we never want event delivery to block the render loop.

---

## 3. Broadcast Channel

The broadcast channel is the primary conduit for discrete events. It provides fan-out semantics: every subscriber sees every event, and events are delivered in causal order.

### 3.1 Channel Configuration

```rust
use tokio::sync::broadcast;

/// The broadcast event channel.
///
/// Capacity: 256 events.
///
/// This capacity handles burst scenarios (e.g., 8 devices connecting
/// simultaneously during discovery, each producing DeviceDiscovered +
/// DeviceConnected events = 16 events in rapid succession). With a
/// 256-slot buffer, subscribers have ample room to process events
/// before lagging.
///
/// At steady-state, the bus sees ~10-30 events/second. The buffer
/// provides ~8-25 seconds of runway for a completely stalled subscriber.
const EVENT_CHANNEL_CAPACITY: usize = 256;

let (sender, _) = broadcast::channel::<TimestampedEvent>(EVENT_CHANNEL_CAPACITY);
```

### 3.2 Lagged Receiver Handling

When a subscriber's internal buffer overflows, `recv()` returns `RecvError::Lagged(n)` where `n` is the number of skipped events. The subscriber must recover gracefully:

```rust
use tokio::sync::broadcast::error::RecvError;

async fn event_consumer_loop(
    mut rx: broadcast::Receiver<TimestampedEvent>,
    state: Arc<DaemonState>,
) {
    loop {
        match rx.recv().await {
            Ok(event) => {
                handle_event(event).await;
            }
            Err(RecvError::Lagged(n)) => {
                tracing::warn!("Event subscriber lagged by {n} events, requesting state snapshot");
                // Rebuild state from a snapshot rather than replaying missed events.
                // The snapshot is authoritative -- missed events don't matter.
                let snapshot = state.snapshot().await;
                handle_state_rebuild(snapshot).await;
            }
            Err(RecvError::Closed) => {
                tracing::info!("Event bus closed, shutting down consumer");
                break;
            }
        }
    }
}
```

**Design rationale:** The event bus does not maintain a replay log. Events are fire-and-forget. This is intentional for a real-time lighting system where the current state matters more than history. When a subscriber lags, a state snapshot provides everything it needs to recover. The snapshot is cheaper and more reliable than replaying potentially hundreds of missed events.

### 3.3 Capacity Tuning

The 256-event capacity is a balance between memory usage and lag tolerance:

| Capacity | Memory (approx) | Lag Runway at 30 events/sec | Notes |
|----------|-----------------|----------------------------|-------|
| 64 | ~32 KB | ~2 seconds | Too tight for burst scenarios |
| 128 | ~64 KB | ~4 seconds | Marginal for slow WebSocket clients |
| **256** | **~128 KB** | **~8 seconds** | Default. Handles bursts and moderate lag. |
| 512 | ~256 KB | ~17 seconds | Excessive for most setups |

If profiling reveals frequent lagging on specific deployments (e.g., high device counts generating many events), the capacity can be increased via configuration:

```toml
[daemon.bus]
event_buffer_size = 512
```

---

## 4. Watch Channels

Watch channels provide latest-value semantics for high-frequency data streams. Unlike broadcast, a watch channel stores exactly one value. When the producer calls `send_replace()`, the old value is dropped. Subscribers calling `changed().await` always get the most recent value, skipping any intermediate values they missed.

### 4.1 FrameData Watch

```rust
use tokio::sync::watch;

/// Latest LED color data for all zones.
/// Published by the render loop at 60fps.
/// Subscribers skip stale frames automatically.
let (frame_tx, frame_rx) = watch::channel(FrameData::empty());
```

See [Section 5](#5-framedata-distribution) for the `FrameData` type definition and binary wire format.

### 4.2 SpectrumData Watch

```rust
/// Latest audio spectrum analysis data.
/// Published by the audio processor at ~30-60fps.
let (spectrum_tx, spectrum_rx) = watch::channel(SpectrumData::empty());
```

See [Section 6](#6-spectrumdata-distribution) for the `SpectrumData` type definition and binary wire format.

### 4.3 ActiveEffectInfo Watch

```rust
/// Current active effect metadata. Changes infrequently (only on effect switch).
/// Used by TUI status bar, web UI header, D-Bus properties.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveEffectInfo {
    pub id: String,
    pub name: String,
    pub engine: String,
    pub controls: HashMap<String, ControlValue>,
    pub audio_reactive: bool,
}

impl ActiveEffectInfo {
    pub fn none() -> Self {
        Self {
            id: String::new(),
            name: "None".into(),
            engine: String::new(),
            controls: HashMap::new(),
            audio_reactive: false,
        }
    }
}

let (effect_tx, effect_rx) = watch::channel(ActiveEffectInfo::none());
```

### 4.4 FpsSnapshot Watch

```rust
/// Current FPS measurements. Updated once per second by the render loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FpsSnapshot {
    pub target: u32,
    pub measured: f32,
    /// Average frame time over the last second, in milliseconds.
    pub avg_frame_time_ms: f32,
    /// Worst-case frame time over the last second, in milliseconds.
    pub max_frame_time_ms: f32,
}

impl FpsSnapshot {
    pub fn default() -> Self {
        Self {
            target: 60,
            measured: 0.0,
            avg_frame_time_ms: 0.0,
            max_frame_time_ms: 0.0,
        }
    }
}

let (fps_tx, fps_rx) = watch::channel(FpsSnapshot::default());
```

### 4.5 Why Watch, Not Broadcast

| Property | `broadcast` | `watch` |
|----------|------------|---------|
| Buffering | N messages (256) | 1 value (latest only) |
| Missed messages | `Lagged(n)` error | Silently skipped |
| Subscriber overhead | Per-subscriber buffer | Single shared value |
| Ideal for | Discrete events (must see each one) | Continuous streams (only latest matters) |
| Backpressure model | Slow subscriber eventually lags | Slow subscriber skips to latest |

Frame data at 60fps means 60 values/second. If the TUI renders at 30fps, it naturally skips every other frame -- exactly what `watch` provides for free. A broadcast channel would waste buffer space on frames the TUI will never read.

---

## 5. FrameData Distribution

LED color data for every zone in the active layout. Published by the render loop at the daemon's frame rate (default 60fps). Subscribers receive only the latest value.

### 5.1 Rust Definition

```rust
/// LED color data for all active zones in the current layout.
/// Published at render frame rate via `watch::Sender`.
#[derive(Debug, Clone)]
pub struct FrameData {
    /// Monotonically increasing frame counter.
    pub frame_number: u32,
    /// Millis since daemon start.
    pub timestamp_ms: u32,
    /// Per-zone color data, ordered consistently with the active layout.
    pub zones: Vec<ZoneColors>,
}

/// Colors for a single device zone.
#[derive(Debug, Clone)]
pub struct ZoneColors {
    /// Stable zone identifier (e.g., "wled_strip_1:zone_0").
    pub zone_id: String,
    /// RGB triplets, one per LED. Length matches the zone's LED count.
    pub colors: Vec<[u8; 3]>,
}

impl FrameData {
    pub fn empty() -> Self {
        Self {
            frame_number: 0,
            timestamp_ms: 0,
            zones: Vec::new(),
        }
    }

    pub fn new(zones: Vec<ZoneColors>, frame_number: u32, timestamp_ms: u32) -> Self {
        Self { frame_number, timestamp_ms, zones }
    }

    /// Total LED count across all zones.
    pub fn total_leds(&self) -> usize {
        self.zones.iter().map(|z| z.colors.len()).sum()
    }
}
```

### 5.2 Binary Wire Format

When transmitted over WebSocket or Unix socket, `FrameData` uses a compact binary encoding to minimize bandwidth. No compression -- the data is already compact and compression would add latency.

```
Binary Frame Layout (type discriminator 0x01):

Offset  Size    Field               Type        Notes
-------------------------------------------------------------
0       1       message_type        u8          Always 0x01
1       4       frame_number        u32 LE      Monotonic counter
5       4       timestamp_ms        u32 LE      Millis since daemon start
9       1       zone_count          u8          Number of zones (max 255)

For each zone (repeated zone_count times):
  +0    2       zone_id_len         u16 LE      Length of zone_id string
  +2    N       zone_id             UTF-8       Zone identifier
  +2+N  2       led_count           u16 LE      Number of LEDs in this zone
  +4+N  M       colors              [u8; M]     RGB triplets (led_count * 3)
```

**Example frame (842 LEDs across 5 zones):**

```
Header:          9 bytes
Zone headers:    5 zones * (~2 + 20 + 2) bytes avg = ~120 bytes
LED data:        842 LEDs * 3 bytes = 2,526 bytes
                 -------------------------------------------------
Total:           ~2,655 bytes per frame

At 30 fps:       ~79.6 KB/s
At 60 fps:       ~159.3 KB/s
```

**Rust serialization:**

```rust
impl FrameData {
    /// Serialize to the compact binary wire format.
    pub fn to_binary(&self) -> Vec<u8> {
        let capacity = 10 + self.zones.iter()
            .map(|z| 4 + z.zone_id.len() + z.colors.len() * 3)
            .sum::<usize>();
        let mut buf = Vec::with_capacity(capacity);

        buf.push(0x01); // message_type
        buf.extend_from_slice(&self.frame_number.to_le_bytes());
        buf.extend_from_slice(&self.timestamp_ms.to_le_bytes());
        buf.push(self.zones.len() as u8);

        for zone in &self.zones {
            let id_bytes = zone.zone_id.as_bytes();
            buf.extend_from_slice(&(id_bytes.len() as u16).to_le_bytes());
            buf.extend_from_slice(id_bytes);
            buf.extend_from_slice(&(zone.colors.len() as u16).to_le_bytes());
            for rgb in &zone.colors {
                buf.extend_from_slice(rgb);
            }
        }

        buf
    }

    /// Deserialize from the binary wire format.
    pub fn from_binary(data: &[u8]) -> Result<Self, DecodeError> {
        if data.len() < 10 || data[0] != 0x01 {
            return Err(DecodeError::InvalidHeader);
        }

        let frame_number = u32::from_le_bytes(data[1..5].try_into()?);
        let timestamp_ms = u32::from_le_bytes(data[5..9].try_into()?);
        let zone_count = data[9] as usize;

        let mut offset = 10;
        let mut zones = Vec::with_capacity(zone_count);

        for _ in 0..zone_count {
            if offset + 2 > data.len() {
                return Err(DecodeError::UnexpectedEof);
            }
            let id_len = u16::from_le_bytes(data[offset..offset + 2].try_into()?) as usize;
            offset += 2;

            if offset + id_len > data.len() {
                return Err(DecodeError::UnexpectedEof);
            }
            let zone_id = String::from_utf8(data[offset..offset + id_len].to_vec())?;
            offset += id_len;

            if offset + 2 > data.len() {
                return Err(DecodeError::UnexpectedEof);
            }
            let led_count = u16::from_le_bytes(data[offset..offset + 2].try_into()?) as usize;
            offset += 2;

            let color_bytes = led_count * 3;
            if offset + color_bytes > data.len() {
                return Err(DecodeError::UnexpectedEof);
            }

            let colors: Vec<[u8; 3]> = data[offset..offset + color_bytes]
                .chunks_exact(3)
                .map(|c| [c[0], c[1], c[2]])
                .collect();
            offset += color_bytes;

            zones.push(ZoneColors { zone_id, colors });
        }

        Ok(Self { frame_number, timestamp_ms, zones })
    }
}
```

### 5.3 Frame Flow: Engine to Devices and Frontends

The render loop publishes frame data to two destinations simultaneously:

```
Render Loop
    |
    +---> Device backends (push_frame)  <-- hardware output, synchronous
    |
    +---> bus.frame.send_replace(...)   <-- watch channel for frontends
```

```rust
// In the render loop -- called every frame (60fps)
pub async fn tick(&mut self) {
    let frame_start = Instant::now();

    // 1. Sample all input sources
    let inputs = self.sample_inputs().await;

    // 2. Render effect -> RGBA canvas buffer
    let canvas = self.effect_engine.render(inputs).await;

    // 3. Spatial mapping: sample canvas at LED positions
    let led_colors = self.spatial_engine.sample(&canvas);

    // 4. Push to all device backends (hardware output)
    for backend in &mut self.backends {
        backend.push_frame(&led_colors).await;
    }

    // 5. Publish frame to event bus (for UI preview)
    let frame_data = FrameData::new(
        led_colors,
        self.frame_counter,
        daemon_uptime_ms() as u32,
    );
    self.bus.frame.send_replace(frame_data);

    // 6. Optionally emit FrameRendered event (debug mode only)
    if self.debug_mode {
        let elapsed = frame_start.elapsed();
        self.bus.emit(HypercolorEvent::FrameRendered {
            frame_number: self.frame_counter,
            timing: FrameTiming {
                render_us: self.last_render_us,
                sample_us: self.last_sample_us,
                push_us: self.last_push_us,
                total_us: elapsed.as_micros() as u32,
                budget_us: 1_000_000 / self.frame_rate,
            },
        });
    }

    self.frame_counter = self.frame_counter.wrapping_add(1);
}
```

**The send_replace is non-blocking.** If no subscribers exist, the frame is silently dropped. If subscribers are slow, they skip to the latest frame on their next `changed().await`. The render loop never waits for frontends.

---

## 6. SpectrumData Distribution

Audio spectrum analysis data published by the FFT processor. Used by the TUI spectrum widget and the web UI audio visualizer.

### 6.1 Rust Definition

```rust
/// Audio spectrum analysis data.
/// Published at the audio analysis rate (typically 30-60fps) via `watch::Sender`.
#[derive(Debug, Clone)]
pub struct SpectrumData {
    /// Millis since daemon start.
    pub timestamp_ms: u32,

    /// Overall audio level (RMS), 0.0 - 1.0.
    pub level: f32,
    /// Low-frequency energy, 0.0 - 1.0.
    pub bass: f32,
    /// Mid-frequency energy, 0.0 - 1.0.
    pub mid: f32,
    /// High-frequency energy, 0.0 - 1.0.
    pub treble: f32,

    /// Beat detected this frame.
    pub beat: bool,
    /// Beat detection confidence, 0.0 - 1.0.
    pub beat_confidence: f32,
    /// Estimated BPM (None if insufficient data).
    pub bpm: Option<f32>,

    /// FFT frequency bins, normalized 0.0 - 1.0.
    /// Full resolution: 200 bins.
    /// Clients request a downsampled count on subscription.
    pub bins: Vec<f32>,
}

impl SpectrumData {
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
    pub fn downsample(&self, target_bins: usize) -> Vec<f32> {
        if self.bins.is_empty() || target_bins == 0 {
            return Vec::new();
        }
        if target_bins >= self.bins.len() {
            return self.bins.clone();
        }
        let chunk_size = self.bins.len() as f32 / target_bins as f32;
        (0..target_bins)
            .map(|i| {
                let start = (i as f32 * chunk_size) as usize;
                let end = ((i + 1) as f32 * chunk_size) as usize;
                let end = end.min(self.bins.len());
                let sum: f32 = self.bins[start..end].iter().sum();
                sum / (end - start) as f32
            })
            .collect()
    }
}
```

### 6.2 Binary Wire Format

```
Binary Spectrum Layout (type discriminator 0x02):

Offset  Size    Field               Type        Notes
-------------------------------------------------------------
0       1       message_type        u8          Always 0x02
1       4       timestamp_ms        u32 LE      Millis since daemon start
5       1       bin_count           u8          Number of frequency bins
6       4       level               f32 LE      Overall RMS level
10      4       bass                f32 LE      Low-frequency energy
14      4       mid                 f32 LE      Mid-frequency energy
18      4       treble              f32 LE      High-frequency energy
22      1       beat                u8          0 = no beat, 1 = beat
23      4       beat_confidence     f32 LE      0.0 - 1.0
27      4       bpm                 f32 LE      Estimated BPM (0.0 = unknown)
31      N*4     bins                [f32 LE]    bin_count frequency bins
```

**Size with 64 bins (typical WebSocket subscription):** `31 + 256 = 287 bytes`
**Size with 200 bins (full resolution):** `31 + 800 = 831 bytes`

At 30fps with 64 bins: **~8.6 KB/s**. At 30fps with 200 bins: **~24.9 KB/s**.

**Rust serialization:**

```rust
impl SpectrumData {
    pub fn to_binary(&self, max_bins: usize) -> Vec<u8> {
        let bins = self.downsample(max_bins);
        let bin_count = bins.len().min(255) as u8;
        let mut buf = Vec::with_capacity(31 + bins.len() * 4);

        buf.push(0x02); // message_type
        buf.extend_from_slice(&self.timestamp_ms.to_le_bytes());
        buf.push(bin_count);
        buf.extend_from_slice(&self.level.to_le_bytes());
        buf.extend_from_slice(&self.bass.to_le_bytes());
        buf.extend_from_slice(&self.mid.to_le_bytes());
        buf.extend_from_slice(&self.treble.to_le_bytes());
        buf.push(self.beat as u8);
        buf.extend_from_slice(&self.beat_confidence.to_le_bytes());
        buf.extend_from_slice(&self.bpm.unwrap_or(0.0).to_le_bytes());
        for bin in &bins {
            buf.extend_from_slice(&bin.to_le_bytes());
        }

        buf
    }

    pub fn from_binary(data: &[u8]) -> Result<Self, DecodeError> {
        if data.len() < 31 || data[0] != 0x02 {
            return Err(DecodeError::InvalidHeader);
        }

        let timestamp_ms = u32::from_le_bytes(data[1..5].try_into()?);
        let bin_count = data[5] as usize;
        let level = f32::from_le_bytes(data[6..10].try_into()?);
        let bass = f32::from_le_bytes(data[10..14].try_into()?);
        let mid = f32::from_le_bytes(data[14..18].try_into()?);
        let treble = f32::from_le_bytes(data[18..22].try_into()?);
        let beat = data[22] != 0;
        let beat_confidence = f32::from_le_bytes(data[23..27].try_into()?);
        let bpm_raw = f32::from_le_bytes(data[27..31].try_into()?);
        let bpm = if bpm_raw > 0.0 { Some(bpm_raw) } else { None };

        let expected_len = 31 + bin_count * 4;
        if data.len() < expected_len {
            return Err(DecodeError::UnexpectedEof);
        }

        let bins: Vec<f32> = data[31..31 + bin_count * 4]
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect();

        Ok(Self {
            timestamp_ms,
            level,
            bass,
            mid,
            treble,
            beat,
            beat_confidence,
            bpm,
            bins,
        })
    }
}
```

---

## 7. Subscription Model

The `HypercolorBus` struct is the central nervous system. It owns all channels and provides typed subscription methods.

### 7.1 EventBus Struct

```rust
use tokio::sync::{broadcast, watch};
use std::sync::Arc;

/// The central event bus. Cloneable -- hand a clone to every subsystem.
///
/// All channel operations are lock-free. The bus is `Send + Sync` and can be
/// shared across arbitrary tokio tasks via `Arc<HypercolorBus>` or direct clone.
#[derive(Clone)]
pub struct HypercolorBus {
    /// Discrete events -- every subscriber sees every event.
    /// Buffer: 256 events. Slow subscribers get `Lagged`.
    events: broadcast::Sender<TimestampedEvent>,

    /// Latest LED color data for all zones.
    pub frame: watch::Sender<FrameData>,

    /// Latest audio spectrum analysis data.
    pub spectrum: watch::Sender<SpectrumData>,

    /// Latest active effect info.
    pub active_effect: watch::Sender<ActiveEffectInfo>,

    /// Latest FPS measurements.
    pub fps: watch::Sender<FpsSnapshot>,

    /// Monotonic clock base for `mono_ms` timestamps.
    start_instant: std::time::Instant,
}

/// An event wrapped with its timestamp. The bus adds the timestamp
/// at publish time -- event producers don't need to worry about clocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimestampedEvent {
    /// ISO 8601 with millisecond precision.
    pub timestamp: String,
    /// Monotonic millis since daemon start (for frame correlation).
    pub mono_ms: u64,
    /// The event payload.
    #[serde(flatten)]
    pub event: HypercolorEvent,
}

impl HypercolorBus {
    /// Create a new bus with default capacities.
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(256);
        let (frame, _) = watch::channel(FrameData::empty());
        let (spectrum, _) = watch::channel(SpectrumData::empty());
        let (active_effect, _) = watch::channel(ActiveEffectInfo::none());
        let (fps, _) = watch::channel(FpsSnapshot::default());

        Self {
            events,
            frame,
            spectrum,
            active_effect,
            fps,
            start_instant: std::time::Instant::now(),
        }
    }

    // ── Publishing ─────────────────────────────────────────────

    /// Publish a discrete event. Timestamp is added automatically.
    /// This is non-blocking -- if no subscribers exist, the event is silently dropped.
    pub fn publish(&self, event: HypercolorEvent) {
        let timestamped = TimestampedEvent {
            timestamp: chrono::Utc::now()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            mono_ms: self.start_instant.elapsed().as_millis() as u64,
            event,
        };
        // Ignore send errors -- they mean no subscribers exist.
        let _ = self.events.send(timestamped);
    }

    /// Convenience alias for `publish` (matches the design doc naming).
    pub fn emit(&self, event: HypercolorEvent) {
        self.publish(event);
    }

    // ── Subscribing ────────────────────────────────────────────

    /// Subscribe to all discrete events (unfiltered).
    pub fn subscribe_all(&self) -> broadcast::Receiver<TimestampedEvent> {
        self.events.subscribe()
    }

    /// Subscribe to discrete events with a filter.
    /// Returns a `FilteredEventReceiver` that only yields matching events.
    pub fn subscribe_filtered(&self, filter: EventFilter) -> FilteredEventReceiver {
        FilteredEventReceiver {
            inner: self.events.subscribe(),
            filter,
        }
    }

    /// Subscribe to frame data (latest-value semantics).
    pub fn subscribe_frames(&self) -> watch::Receiver<FrameData> {
        self.frame.subscribe()
    }

    /// Subscribe to spectrum data (latest-value semantics).
    pub fn subscribe_spectrum(&self) -> watch::Receiver<SpectrumData> {
        self.spectrum.subscribe()
    }

    /// Subscribe to active effect info (latest-value semantics).
    pub fn subscribe_active_effect(&self) -> watch::Receiver<ActiveEffectInfo> {
        self.active_effect.subscribe()
    }

    /// Subscribe to FPS snapshots (latest-value semantics).
    pub fn subscribe_fps(&self) -> watch::Receiver<FpsSnapshot> {
        self.fps.subscribe()
    }

    /// Number of active broadcast subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.events.receiver_count()
    }
}
```

### 7.2 EventFilter

```rust
/// Filter for selective event subscription.
/// All fields are optional. If a field is `None`, it matches everything.
/// Multiple fields combine with AND logic.
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    /// Only receive events in these categories.
    pub categories: Option<Vec<EventCategory>>,

    /// Exclude events of these specific types (by variant name).
    pub exclude_types: Option<Vec<String>>,

    /// Only receive events mentioning these device IDs.
    pub device_ids: Option<Vec<String>>,

    /// Minimum priority level. Events below this priority are dropped.
    pub min_priority: Option<EventPriority>,
}

impl EventFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn categories(mut self, cats: Vec<EventCategory>) -> Self {
        self.categories = Some(cats);
        self
    }

    pub fn exclude_types(mut self, types: Vec<String>) -> Self {
        self.exclude_types = Some(types);
        self
    }

    pub fn device_ids(mut self, ids: Vec<String>) -> Self {
        self.device_ids = Some(ids);
        self
    }

    pub fn min_priority(mut self, priority: EventPriority) -> Self {
        self.min_priority = Some(priority);
        self
    }

    /// Test whether an event passes this filter.
    pub fn matches(&self, event: &TimestampedEvent) -> bool {
        // Category filter
        if let Some(ref cats) = self.categories {
            if !cats.contains(&event.event.category()) {
                return false;
            }
        }

        // Type exclusion filter
        if let Some(ref exclude) = self.exclude_types {
            let type_name = event.event.type_name();
            if exclude.iter().any(|t| t == type_name) {
                return false;
            }
        }

        // Device ID filter
        if let Some(ref ids) = self.device_ids {
            if let Some(device_id) = event.event.device_id() {
                if !ids.iter().any(|id| id == device_id) {
                    return false;
                }
            }
            // Events without a device_id pass the device filter
            // (they're system/effect events, not device-specific).
        }

        // Priority filter
        if let Some(min) = self.min_priority {
            if event.event.priority() < min {
                return false;
            }
        }

        true
    }
}
```

### 7.3 FilteredEventReceiver

```rust
/// A broadcast receiver that applies a filter to incoming events.
/// Events that don't match the filter are silently consumed and discarded.
pub struct FilteredEventReceiver {
    inner: broadcast::Receiver<TimestampedEvent>,
    filter: EventFilter,
}

impl FilteredEventReceiver {
    /// Receive the next event that passes the filter.
    /// Blocks until a matching event arrives or the channel closes.
    pub async fn recv(&mut self) -> Result<TimestampedEvent, broadcast::error::RecvError> {
        loop {
            let event = self.inner.recv().await?;
            if self.filter.matches(&event) {
                return Ok(event);
            }
            // Non-matching events are silently consumed.
        }
    }
}
```

### 7.4 Usage Patterns

**Render loop publishes frame data:**

```rust
// In the render loop -- called every frame (60fps)
let led_colors = spatial_engine.sample(&canvas);
bus.frame.send_replace(FrameData::new(&led_colors, frame_num, ts));
```

**Audio processor publishes spectrum data:**

```rust
// In the audio processing task -- called at audio frame rate
let spectrum = fft_processor.analyze(&audio_buffer);
bus.spectrum.send_replace(SpectrumData::from(spectrum));
```

**Device manager publishes events:**

```rust
// When a device connects
bus.publish(HypercolorEvent::DeviceConnected {
    device_id: device.id.clone(),
    name: device.name.clone(),
    backend: "wled".into(),
    led_count: device.led_count,
    zones: device.zones.iter().map(ZoneRef::from).collect(),
});
```

**WebSocket handler subscribes to everything:**

```rust
let mut events = bus.subscribe_all();
let mut frames = bus.subscribe_frames();
let mut spectrum = bus.subscribe_spectrum();

loop {
    tokio::select! {
        Ok(event) = events.recv() => {
            ws.send(Message::Text(serde_json::to_string(&event)?)).await?;
        }
        Ok(()) = frames.changed() => {
            let frame = frames.borrow_and_update().clone();
            ws.send(Message::Binary(frame.to_binary())).await?;
        }
        Ok(()) = spectrum.changed() => {
            let spec = spectrum.borrow_and_update().clone();
            ws.send(Message::Binary(spec.to_binary(64))).await?;
        }
    }
}
```

**TUI subscribes with device filter:**

```rust
let filter = EventFilter::new()
    .categories(vec![EventCategory::Device, EventCategory::Effect, EventCategory::System])
    .exclude_types(vec!["FrameRendered".into(), "BeatDetected".into()]);

let mut events = bus.subscribe_filtered(filter);
let mut frames = bus.subscribe_frames();
```

---

## 8. IPC Protocol for TUI/CLI

The TUI and CLI communicate with the daemon over a local socket. The protocol is length-prefixed JSON-RPC 2.0 with streaming extensions for event subscriptions and binary data channels.

### 8.1 Transport Selection

| Platform | Primary IPC | Path / Name | Fallback |
|----------|------------|-------------|----------|
| **Linux** | Unix domain socket | `/run/hypercolor/hypercolor.sock` | TCP `127.0.0.1:9421` |
| **macOS** | Unix domain socket | `/tmp/hypercolor/hypercolor.sock` | TCP `127.0.0.1:9421` |
| **Windows** | Named pipe | `\\.\pipe\hypercolor` | TCP `127.0.0.1:9421` |
| **Remote** | TCP | `<host>:9421` | (no fallback) |

Transport selection is automatic. The client tries in order: platform-native IPC, then TCP fallback. The `--socket` and `--host` flags override auto-detection.

### 8.2 Framing

Every message on the IPC socket is length-prefixed:

```
+----------------------+---------------------------------------+
|  Length (4 bytes)     |  Payload (Length bytes)                |
|  u32 little-endian   |  UTF-8 JSON or raw binary             |
+----------------------+---------------------------------------+
```

**Length** is the byte count of the payload, not including the 4-byte length prefix itself.

**Payload type** is determined by the first byte:
- `0x7B` (`{`) -- JSON-RPC message (text)
- `0x01` -- Binary frame data
- `0x02` -- Binary spectrum data

This allows multiplexing JSON-RPC requests/responses with binary streaming data on the same socket connection.

**Maximum message size:** 16 MiB. Any message exceeding this is rejected with a connection error.

### 8.3 JSON-RPC 2.0 Messages

All command/response interactions use strict JSON-RPC 2.0. The IPC protocol is request-response (client sends request, daemon sends response) plus server-initiated notifications for subscriptions.

#### Request

```json
{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "effects.apply",
    "params": {
        "effect_id": "aurora",
        "controls": { "effectSpeed": 70 },
        "transition_ms": 500
    }
}
```

`id` is a client-chosen integer or string. The daemon echoes it in the response for correlation.

#### Response (success)

```json
{
    "jsonrpc": "2.0",
    "id": 1,
    "result": {
        "effect": {
            "id": "aurora",
            "name": "Aurora"
        },
        "applied_controls": { "effectSpeed": 70 }
    }
}
```

#### Response (error)

```json
{
    "jsonrpc": "2.0",
    "id": 1,
    "error": {
        "code": -32602,
        "message": "Effect 'nonexistent' not found",
        "data": {
            "available": ["aurora", "rainbow-wave", "solid-color"]
        }
    }
}
```

#### Server-Initiated Notification (no `id`)

```json
{
    "jsonrpc": "2.0",
    "method": "event",
    "params": {
        "type": "EffectStarted",
        "timestamp": "2026-03-01T20:32:01.482Z",
        "data": {
            "effect": { "id": "aurora", "name": "Aurora", "engine": "servo" },
            "trigger": "user",
            "previous": { "id": "rainbow", "name": "Rainbow Wave", "engine": "wgpu" },
            "transition": { "transition_type": "crossfade", "duration_ms": 500 }
        }
    }
}
```

### 8.4 JSON-RPC Error Codes

Standard JSON-RPC 2.0 error codes, plus Hypercolor-specific application errors.

| Code | Meaning | When |
|------|---------|------|
| `-32700` | Parse error | Malformed JSON |
| `-32600` | Invalid request | Missing required JSON-RPC fields |
| `-32601` | Method not found | Unknown RPC method |
| `-32602` | Invalid params | Params fail validation |
| `-32603` | Internal error | Unexpected daemon error |
| `-1` | Not found | Resource (effect, device, profile) does not exist |
| `-2` | Conflict | State conflict (e.g., device already connected) |
| `-3` | Unavailable | Daemon is starting up or shutting down |
| `-4` | Timeout | Backend operation timed out |
| `-5` | Subscription error | Invalid channel or filter |

### 8.5 RPC Method Catalog

Complete catalog of JSON-RPC methods. Parameters marked with `?` are optional.

#### State Methods

```
state.get
    params:  {}
    result:  { running, paused, brightness, fps: { target, actual },
               effect: { id, name }, profile?: { id, name },
               layout: { id, name }, devices: { connected, total_leds },
               inputs: { audio, screen }, uptime_seconds }
```

```
state.health
    params:  {}
    result:  { status, version, uptime_seconds,
               checks: { render_loop, device_backends, event_bus } }
```

```
state.set_brightness
    params:  { brightness: u8 }     // 0-100
    result:  { brightness: u8 }
```

```
state.set_fps
    params:  { fps: u32 }          // 1-144
    result:  { fps: u32 }
```

```
state.pause
    params:  {}
    result:  { paused: true }
```

```
state.resume
    params:  {}
    result:  { paused: false }
```

#### Effect Methods

```
effects.list
    params:  { query?: string, category?: string,
               audio_reactive?: bool, engine?: string,
               offset?: u32, limit?: u32 }
    result:  { items: [EffectSummary], pagination: { offset, limit, total, has_more } }
```

```
effects.get
    params:  { effect_id: string }
    result:  { id, name, description, author, engine, category, tags,
               audio_reactive, controls: [ControlDefinition], presets: [PresetSummary] }
```

```
effects.apply
    params:  { effect_id: string, controls?: object, transition_ms?: u32 }
    result:  { effect: EffectRef, applied_controls: object }
```

```
effects.current
    params:  {}
    result:  { id, name, engine, controls: { [id]: current_value } }
```

```
effects.set_controls
    params:  { controls: object }
    result:  { updated: { [id]: new_value } }
```

```
effects.next
    params:  {}
    result:  { effect: EffectRef }
```

```
effects.previous
    params:  {}
    result:  { effect: EffectRef }
```

```
effects.shuffle
    params:  {}
    result:  { effect: EffectRef }
```

#### Device Methods

```
devices.list
    params:  { status?: "all" | "connected" | "disconnected" }
    result:  { items: [DeviceInfo] }
```

```
devices.get
    params:  { device_id: string }
    result:  DeviceInfo  // full detail including zones, connection, metadata
```

```
devices.discover
    params:  { backends?: [string] }    // default: all enabled backends
    result:  { scan_id: string, status: "scanning" }
    // Results arrive as DeviceDiscoveryCompleted events
```

```
devices.enable
    params:  { device_id: string }
    result:  { device_id, enabled: true }
```

```
devices.disable
    params:  { device_id: string }
    result:  { device_id, enabled: false }
```

```
devices.rename
    params:  { device_id: string, name: string }
    result:  { device_id, name }
```

```
devices.test
    params:  { device_id: string, pattern?: "sweep" | "flash" | "chase" }
    result:  { status: "testing" }
    // Test completion arrives as an event
```

#### Profile Methods

```
profiles.list
    params:  {}
    result:  { items: [{ id, name, created_at, updated_at, is_active }] }
```

```
profiles.get
    params:  { profile_id: string }
    result:  ProfileDetail  // full profile with effect, controls, device overrides
```

```
profiles.apply
    params:  { profile_id: string, transition_ms?: u32 }
    result:  { profile: { id, name }, applied: true }
```

```
profiles.save
    params:  { name: string, description?: string }
    result:  { profile_id, name, created_at }
```

```
profiles.delete
    params:  { profile_id: string }
    result:  { deleted: true }
```

#### Layout Methods

```
layouts.list
    params:  {}
    result:  { items: [{ id, name, zone_count, is_active }] }
```

```
layouts.get
    params:  { layout_id: string }
    result:  LayoutDetail  // full layout with all zone positions
```

```
layouts.apply
    params:  { layout_id: string }
    result:  { layout: { id, name }, applied: true }
```

#### Scene Methods

```
scenes.list
    params:  {}
    result:  { items: [{ id, name, enabled, profile_id, trigger_type }] }
```

```
scenes.activate
    params:  { scene_id: string }
    result:  { scene: { id, name }, activated: true }
```

```
scenes.set_enabled
    params:  { scene_id: string, enabled: bool }
    result:  { scene_id, enabled }
```

#### Input Methods

```
inputs.list
    params:  {}
    result:  { items: [InputSourceInfo] }
```

```
inputs.audio_config
    params:  { config?: AudioConfig }   // omit to get, provide to set
    result:  AudioConfig
```

#### Subscription Methods

```
subscribe
    params:  {
        channels: ["events" | "frames" | "spectrum"],
        config?: {
            events?: {
                categories?: [string],      // filter by event category
                exclude_types?: [string],   // exclude specific event types
                device_ids?: [string]       // filter to specific devices
            },
            frames?: {
                fps?: u32,                  // max frame delivery rate (default: 30)
                zones?: [string] | "all"    // which zones to include
            },
            spectrum?: {
                fps?: u32,                  // max delivery rate (default: 30)
                bins?: u8                   // downsampled bin count (default: 64)
            }
        }
    }
    result:  { subscribed: [string] }
```

```
unsubscribe
    params:  { channels: [string] }
    result:  { unsubscribed: [string] }
```

#### Shell Completion Methods

```
completions
    params:  { resource: "effects" | "devices" | "profiles" | "layouts" | "scenes",
               prefix?: string }
    result:  { completions: [{ value: string, description?: string }] }
```

### 8.6 Subscription Lifecycle

When a TUI or CLI subscribes to streaming channels, the daemon begins sending server-initiated messages on the same socket.

**Sequence diagram:**

```
CLI/TUI                                    Daemon
  |                                          |
  |<-- { method: "hello", params: ... } -----|  (unsolicited, on connect)
  |                                          |
  |--- { method: "state.get" } ------------>|
  |<-- { result: { ... full state ... } } --|
  |                                          |
  |--- { method: "subscribe",               |
  |     params: { channels: ["events",       |
  |       "frames", "spectrum"],             |
  |       config: { frames: { fps: 30 },     |
  |                 spectrum: { bins: 64 } } |
  |   } } --------------------------------->|
  |<-- { result: { subscribed: [...] } } ---|
  |                                          |
  |        (daemon now streams to client)    |
  |                                          |
  |<-- [4-byte len][0x01][binary frame] ----|  (every 33ms)
  |<-- [4-byte len][0x02][binary spectrum] -|  (every 33ms)
  |<-- [4-byte len][JSON notification] -----|  (on events)
  |                                          |
  |--- { method: "effects.apply",           |
  |     params: { effect_id: "aurora" } } ->|
  |<-- { result: { ... } } ----------------|
  |                                          |
  |<-- [JSON: EffectStarted event] ---------|  (triggered by the apply)
  |                                          |
  |--- { method: "unsubscribe",             |
  |     params: { channels: ["spectrum"] } }|
  |<-- { result: { unsubscribed: [...] } } -|
  |                                          |
  |        (spectrum stops, frames continue) |
```

**Frame rate throttling:** The daemon applies a rate limiter per subscription. If the client subscribes to frames at 30fps but the daemon runs at 60fps, the daemon drops every other frame for that client. The `watch` channel naturally handles this -- the daemon only sends when the previous send's interval has elapsed.

### 8.7 Connection Lifecycle

1. **Connect** -- Client opens Unix socket / named pipe / TCP connection.
2. **Hello** -- Daemon sends an unsolicited notification:

```json
{
    "jsonrpc": "2.0",
    "method": "hello",
    "params": {
        "version": "1.0",
        "daemon_version": "0.1.0",
        "capabilities": ["events", "frames", "spectrum", "commands"],
        "pid": 4821
    }
}
```

3. **Operate** -- Client sends requests, receives responses. Optionally subscribes to streaming channels.
4. **Disconnect** -- Either side closes the connection. The daemon cleans up all subscriptions for that client. No explicit disconnect message required.
5. **Reconnect** -- On reconnect, the client must re-subscribe to any channels. The daemon sends a fresh `hello`. No state is carried over from the previous session.

### 8.8 Cross-Platform IPC Abstraction

```rust
use tokio::io::{AsyncRead, AsyncWrite};

/// A bidirectional IPC stream. Wraps Unix socket, named pipe, or TCP.
pub enum IpcStream {
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
    #[cfg(windows)]
    Pipe(tokio::net::windows::named_pipe::NamedPipeClient),
    Tcp(tokio::net::TcpStream),
}

impl AsyncRead for IpcStream { /* delegate to inner */ }
impl AsyncWrite for IpcStream { /* delegate to inner */ }

/// Platform-adaptive IPC listener.
pub enum IpcListener {
    #[cfg(unix)]
    Unix(tokio::net::UnixListener),
    #[cfg(windows)]
    Pipe(/* named pipe server */),
    Tcp(tokio::net::TcpListener),
}

impl IpcListener {
    /// Create a listener using the best available transport.
    pub async fn bind(config: &IpcConfig) -> Result<Self> {
        #[cfg(unix)]
        {
            match tokio::net::UnixListener::bind(&config.socket_path) {
                Ok(listener) => return Ok(Self::Unix(listener)),
                Err(e) => tracing::warn!("Unix socket failed, falling back to TCP: {e}"),
            }
        }

        #[cfg(windows)]
        {
            match create_named_pipe(&config.pipe_name) {
                Ok(server) => return Ok(Self::Pipe(server)),
                Err(e) => tracing::warn!("Named pipe failed, falling back to TCP: {e}"),
            }
        }

        let addr = format!("127.0.0.1:{}", config.tcp_port);
        Ok(Self::Tcp(tokio::net::TcpListener::bind(&addr).await?))
    }
}
```

### 8.9 Transport Comparison

| Property | Unix Socket | Named Pipe | TCP Localhost |
|----------|------------|------------|---------------|
| **Latency** | ~2us | ~5us | ~30us |
| **Throughput** | Kernel-limited | Kernel-limited | ~1 Gbps |
| **Auth** | File permissions + `SO_PEERCRED` | Security descriptors | None (localhost) or API key |
| **Firewall** | Not affected | Not affected | May be blocked |
| **Remote** | No | No | Yes |
| **Cleanup** | Must remove stale socket file | Automatic | Automatic |
| **Max connections** | ulimit (typically 1024+) | `max_instances` (configurable) | ulimit |
| **Container-friendly** | Volume mount required | N/A | Yes |

---

## 9. WebSocket Bridge

The WebSocket at `ws://127.0.0.1:9420/api/v1/ws` is the primary real-time channel for the SvelteKit web frontend. It carries JSON messages for commands and events, plus binary messages for frame and spectrum data.

### 9.1 Connection & Handshake

```
GET /api/v1/ws HTTP/1.1
Host: 127.0.0.1:9420
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Key: ...
Sec-WebSocket-Protocol: hypercolor-v1
```

The `hypercolor-v1` subprotocol is requested by the client to ensure version compatibility. If the daemon supports a different version, it can negotiate by responding with its highest compatible version.

**On successful upgrade, the daemon sends a `hello` message (JSON text frame):**

```json
{
    "type": "hello",
    "version": "1.0",
    "state": {
        "running": true,
        "paused": false,
        "brightness": 85,
        "fps": { "target": 60, "actual": 59.7 },
        "effect": { "id": "aurora", "name": "Aurora", "engine": "servo" },
        "profile": { "id": "chill", "name": "Chill Mode" },
        "layout": { "id": "main_setup", "name": "Main Desk Setup" },
        "devices": {
            "connected": 5,
            "total_leds": 842,
            "list": [
                { "id": "wled_strip_1", "name": "WLED Living Room", "leds": 120, "status": "connected" },
                { "id": "prism8_case", "name": "Prism 8 Controller", "leds": 1008, "status": "connected" }
            ]
        },
        "inputs": {
            "audio": { "enabled": true, "source": "PipeWire" },
            "screen": { "enabled": false }
        }
    },
    "capabilities": ["frames", "spectrum", "canvas", "events", "commands", "metrics"]
}
```

This `hello` is a complete state snapshot. A reconnecting client can rebuild its entire UI from this single message.

### 9.2 Subscription-Based Filtering

By default, only `events` is subscribed. The client explicitly subscribes to high-bandwidth channels.

**Subscribe:**

```json
{
    "type": "subscribe",
    "channels": ["frames", "spectrum"],
    "config": {
        "frames": {
            "fps": 30,
            "format": "binary",
            "zones": "all"
        },
        "spectrum": {
            "fps": 30,
            "bins": 64
        }
    }
}
```

**Subscription acknowledgment:**

```json
{
    "type": "subscribed",
    "channels": ["frames", "spectrum"],
    "config": {
        "frames": { "fps": 30, "format": "binary", "zones": "all" },
        "spectrum": { "fps": 30, "bins": 64 }
    }
}
```

**Unsubscribe:**

```json
{
    "type": "unsubscribe",
    "channels": ["canvas"]
}
```

**Available channels:**

| Channel | Data Type | Default FPS | Description |
|---------|-----------|-------------|-------------|
| `frames` | Binary (0x01) | 30 | Per-zone LED colors |
| `spectrum` | Binary (0x02) | 30 | Audio FFT spectrum |
| `canvas` | Binary (0x03) | 15 | Raw canvas pixels (640x480 default, configurable) |
| `events` | JSON | N/A (push) | Discrete state change events |
| `metrics` | JSON | 1 | Performance metrics |

### 9.3 Binary Frame Mode for LED Data

Binary WebSocket messages use the same encoding as the IPC protocol (sections 5.2 and 6.2). The first byte discriminates the message type.

| Type Byte | Format | Description |
|-----------|--------|-------------|
| `0x01` | Frame binary | LED colors for all zones (see section 5.2) |
| `0x02` | Spectrum binary | Audio spectrum data (see section 6.2) |
| `0x03` | Canvas binary | Raw canvas pixel data |

**Canvas message (type 0x03):**

```
Offset  Size    Field               Type        Notes
-------------------------------------------------------------
0       1       message_type        u8          Always 0x03
1       4       frame_number        u32 LE      Monotonic counter
5       4       timestamp_ms        u32 LE      Millis since daemon start
9       2       width               u16 LE      Canvas width (320)
11      2       height              u16 LE      Canvas height (200)
13      1       format              u8          0 = RGB, 1 = RGBA
14      N       pixels              [u8]        width * height * bpp bytes
```

**Canvas sizes:**
- RGB (format 0): `14 + 320 * 200 * 3 = 192,014 bytes`
- RGBA (format 1): `14 + 320 * 200 * 4 = 256,014 bytes`

At 15fps (RGB): **~2.8 MB/s**. Only subscribe when the spatial editor is open.

### 9.4 JSON Event Messages

All JSON messages from the server use a consistent envelope:

```json
{
    "type": "event",
    "event": "EffectStarted",
    "timestamp": "2026-03-01T12:00:00.123Z",
    "data": {
        "effect": { "id": "aurora", "name": "Aurora", "engine": "servo" },
        "trigger": "user",
        "previous": { "id": "rainbow", "name": "Rainbow Wave", "engine": "wgpu" },
        "transition": { "transition_type": "crossfade", "duration_ms": 500 }
    }
}
```

The `event` field matches the `HypercolorEvent` variant name. The `data` field matches the variant's inner struct.

**Metrics messages (on `metrics` channel):**

```json
{
    "type": "metrics",
    "timestamp": "2026-03-01T12:00:01.000Z",
    "data": {
        "fps": { "target": 60, "actual": 59.7 },
        "frame_time_ms": { "render": 12.3, "sample": 0.4, "push": 1.8, "total": 14.5 },
        "memory_mb": 42,
        "device_latency": {
            "wled_strip_1": 0.8,
            "prism8_case": 2.1
        },
        "event_bus": {
            "subscribers": 3,
            "events_per_sec": 12
        }
    }
}
```

### 9.5 Commands (Client to Server)

Clients can issue commands over the WebSocket instead of separate REST requests. This avoids the overhead of establishing HTTP connections for UI interactions that already have an open WebSocket.

**Command message:**

```json
{
    "type": "command",
    "id": "cmd_001",
    "method": "POST",
    "path": "/effects/aurora/apply",
    "body": {
        "controls": { "effectSpeed": 70 },
        "transition": { "type": "crossfade", "duration_ms": 500 }
    }
}
```

**Command response:**

```json
{
    "type": "response",
    "id": "cmd_001",
    "status": 200,
    "data": {
        "effect": { "id": "aurora", "name": "Aurora" },
        "applied_controls": { "effectSpeed": 70 }
    }
}
```

The `id` field is client-chosen and echoed in the response for correlation. The `method` and `path` fields mirror the REST API surface. This means the web UI has a single transport (WebSocket) for everything -- state sync, streaming data, and commands.

### 9.6 Ping / Pong & Keepalive

The WebSocket connection uses standard RFC 6455 ping/pong frames. The daemon sends a ping every **30 seconds**. If no pong is received within **10 seconds**, the connection is considered dead and closed.

Additionally, the daemon sends a lightweight JSON heartbeat on the `events` channel every 60 seconds if no other events have been emitted:

```json
{
    "type": "heartbeat",
    "timestamp": "2026-03-01T12:01:00.000Z",
    "uptime_seconds": 86460
}
```

### 9.7 Reconnection Strategy

The web UI implements exponential backoff reconnection:

```
Attempt 1: immediate (0ms)
Attempt 2: 500ms delay
Attempt 3: 1,000ms delay
Attempt 4: 2,000ms delay
Attempt 5: 4,000ms delay
Attempt 6+: 8,000ms delay (capped)
Reset: on successful hello
```

**On reconnect:**

1. Establish new WebSocket connection
2. Receive `hello` with full state snapshot
3. Re-send `subscribe` for previously active channels
4. Rebuild UI state from the `hello` snapshot
5. Resume receiving frames/spectrum from the current point

No event replay. The `hello` snapshot is the only recovery mechanism needed -- LED lighting is a real-time system where history doesn't matter.

### 9.8 Compression

`permessage-deflate` (RFC 7692) is enabled for JSON text messages. Typical event messages compress well (50-70% reduction).

Binary messages (frames, spectrum, canvas) are sent **uncompressed**. They're already dense data with no redundancy, and the compression overhead would add ~1ms latency per message -- unacceptable for 30-60fps streams.

### 9.9 WebSocket Message Summary

| Direction | Type | Format | Message `type` Field |
|-----------|------|--------|----------------------|
| S -> C | State snapshot | JSON text | `hello` |
| S -> C | Discrete event | JSON text | `event` |
| S -> C | Performance metrics | JSON text | `metrics` |
| S -> C | Heartbeat | JSON text | `heartbeat` |
| S -> C | LED frame data | Binary | `0x01` first byte |
| S -> C | Spectrum data | Binary | `0x02` first byte |
| S -> C | Canvas data | Binary | `0x03` first byte |
| S -> C | Command response | JSON text | `response` |
| S -> C | Subscription ack | JSON text | `subscribed` |
| C -> S | Subscribe | JSON text | `subscribe` |
| C -> S | Unsubscribe | JSON text | `unsubscribe` |
| C -> S | Command (REST over WS) | JSON text | `command` |

---

## 10. Event Serialization

### 10.1 JSON (serde) -- Default Format

All `HypercolorEvent` variants derive `Serialize` and `Deserialize` via serde. The `#[serde(tag = "type", content = "data")]` attribute produces externally tagged JSON:

```json
{
    "type": "DeviceConnected",
    "data": {
        "device_id": "wled_strip_1",
        "name": "WLED Living Room",
        "backend": "wled",
        "led_count": 120,
        "zones": [{ "zone_id": "zone_0", "device_id": "wled_strip_1", "topology": "strip", "led_count": 120 }]
    }
}
```

The `TimestampedEvent` wrapper uses `#[serde(flatten)]` so the timestamp fields merge with the event:

```json
{
    "timestamp": "2026-03-01T20:32:01.482Z",
    "mono_ms": 3721482,
    "type": "DeviceConnected",
    "data": { ... }
}
```

**JSON is used for:** WebSocket event messages, IPC JSON-RPC notifications, MQTT payloads, REST API event streams, `hypercolor watch` CLI output, debug logging.

### 10.2 Compact Binary -- High-Frequency Events

For high-frequency data (frame data, spectrum data), JSON is too verbose. These use custom binary wire formats defined in Sections 5.2 and 6.2.

For the event bus itself (the `tokio::broadcast` channel), events are passed as Rust structs -- no serialization overhead. Serialization only happens at the boundary when events cross process or network boundaries.

**Serialization strategy by transport:**

| Transport | Event Format | Frame/Spectrum Format |
|-----------|-------------|----------------------|
| In-process (bus) | Native Rust struct (zero-cost) | Native Rust struct |
| IPC (Unix socket / named pipe) | JSON-RPC (serde_json) | Custom binary (0x01/0x02) |
| WebSocket | JSON text frame | Binary WebSocket frame (0x01/0x02/0x03) |
| D-Bus | D-Bus signal marshaling (zbus) | N/A (not streamed over D-Bus) |
| MQTT | JSON | N/A (not streamed over MQTT) |

### 10.3 Bincode for Persistent Event Log

When the daemon's optional event log is enabled (`--event-log /tmp/hypercolor-events.log`), events are written to a ring buffer file. For space efficiency, the log uses `bincode` serialization:

```rust
use bincode;

/// Append an event to the ring buffer log file.
fn log_event(event: &TimestampedEvent, writer: &mut impl std::io::Write) -> Result<()> {
    let encoded = bincode::serialize(event)?;
    // Write length-prefixed for easy sequential reading
    writer.write_all(&(encoded.len() as u32).to_le_bytes())?;
    writer.write_all(&encoded)?;
    Ok(())
}

/// Read events from the log file.
fn read_events(reader: &mut impl std::io::Read) -> Result<Vec<TimestampedEvent>> {
    let mut events = Vec::new();
    loop {
        let mut len_buf = [0u8; 4];
        match reader.read_exact(&mut len_buf) {
            Ok(()) => {},
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf)?;
        events.push(bincode::deserialize(&buf)?);
    }
    Ok(events)
}
```

**Bincode vs JSON size comparison for a `DeviceConnected` event:**
- JSON: ~280 bytes
- Bincode: ~85 bytes (~70% smaller)

The ring buffer retains the last 10,000 events (configurable). At ~85 bytes/event, that's ~850 KB -- negligible.

### 10.4 Binary Message Type Registry

All binary messages on both IPC and WebSocket use a single-byte type discriminator at offset 0.

| Type Byte | Message | Direction |
|-----------|---------|-----------|
| `0x01` | LED frame data (`FrameData`) | Server -> Client |
| `0x02` | Audio spectrum data (`SpectrumData`) | Server -> Client |
| `0x03` | Canvas pixel data | Server -> Client |
| `0x04`-`0x0F` | Reserved for future binary streams | -- |
| `0x7B` (`{`) | JSON-RPC / JSON message | Both |

Bytes `0x10`-`0x7A` and `0x7C`-`0xFF` are reserved for future use. Any message with an unrecognized type byte should be silently ignored by the receiver.

---

## 11. Backpressure

### 11.1 Event Dropping Policy

The dual-channel design (`broadcast` for events, `watch` for frames/spectrum) inherently handles backpressure differently per data type.

**Events (`broadcast`):**

When a subscriber's receive buffer fills (256 events), `recv()` returns `Lagged(n)`. The subscriber must request a state snapshot to recover. No events are replayed. This provides natural backpressure without blocking publishers.

**Frames and Spectrum (`watch`):**

The `watch` channel stores exactly one value. When the render loop calls `send_replace()`, the old value is dropped. Subscribers calling `changed().await` always get the latest frame. If a subscriber is slow, it simply skips frames -- exactly the behavior we want for video-like data. There is no backpressure path back to the render loop.

### 11.2 Priority Events That Never Drop

Critical events (`DaemonShutdown`, `ShutdownRequested`, `Error(Critical)`) must reach every subscriber regardless of backpressure state. The implementation uses a dedicated side-channel:

```rust
/// Critical events bypass the main broadcast channel.
/// A small watch channel ensures at least the most recent critical event is available.
pub struct CriticalEventChannel {
    /// The most recent critical event (watch semantics -- never lost).
    latest: watch::Sender<Option<TimestampedEvent>>,
}

impl HypercolorBus {
    pub fn publish(&self, event: HypercolorEvent) {
        let timestamped = self.wrap_timestamp(event.clone());

        // Always publish to the main broadcast channel.
        let _ = self.events.send(timestamped.clone());

        // For critical events, also publish to the side-channel.
        if event.priority() == EventPriority::Critical {
            self.critical.latest.send_replace(Some(timestamped));
        }
    }
}
```

Subscribers that detect `Lagged` should check the critical event channel before requesting a state snapshot:

```rust
Err(RecvError::Lagged(n)) => {
    // Check if we missed any critical events.
    let critical = bus.subscribe_critical().borrow().clone();
    if let Some(critical_event) = critical {
        handle_critical(critical_event).await;
    }
    // Then rebuild from snapshot.
    let snapshot = state.snapshot().await;
    handle_state_rebuild(snapshot).await;
}
```

### 11.3 Per-Transport Rate Limiting

Each transport applies its own rate limits to outbound streaming data:

| Transport | Frame Rate Limit | Spectrum Rate Limit | Event Burst Limit |
|-----------|-----------------|--------------------|--------------------|
| WebSocket | Client-configurable (default 30fps) | Client-configurable (default 30fps) | 100 events/sec |
| IPC (TUI) | Client-configurable (default 30fps) | Client-configurable (default 30fps) | No limit (local) |
| IPC (CLI watch) | Client-configurable (default 10fps) | Client-configurable (default 10fps) | No limit (local) |
| D-Bus signals | N/A (events only) | N/A | 50 signals/sec |

When a transport's outbound queue fills, the daemon drops the oldest undelivered message for that transport. This is per-connection -- one slow WebSocket client doesn't affect others.

### 11.4 What Happens When Subscribers Can't Keep Up

| Scenario | Behavior | Recovery |
|----------|----------|----------|
| Slow WebSocket client, frame data | Frames are dropped; client always gets the latest frame on its next read | Automatic (watch semantics) |
| Slow WebSocket client, events | Client receives `Lagged` notification in the event stream | Client re-subscribes and gets a `hello` state snapshot |
| TUI rendering slower than daemon | TUI skips frames transparently; LED preview shows latest data | Automatic |
| CLI `watch` mode falls behind | Events are dropped; CLI prints warning | User adjusts filter or lowers rate |
| D-Bus signal queue full | Oldest signals are dropped by the D-Bus daemon | Desktop consumer re-reads properties |

---

## 12. Thread Safety

### 12.1 Send + Sync Bounds

`HypercolorBus` is `Clone + Send + Sync`. All its internal channel types (`broadcast::Sender`, `watch::Sender`) are `Send + Sync` and use atomic operations internally -- no mutex, no lock.

```rust
// HypercolorBus is automatically Send + Sync because all its fields are:
//   - broadcast::Sender<T>: Send + Sync where T: Send
//   - watch::Sender<T>: Send + Sync where T: Send + Sync
//   - std::time::Instant: Send + Sync
//
// TimestampedEvent is Send + Sync because HypercolorEvent is.
// HypercolorEvent is Send + Sync because all its fields are:
//   - String: Send + Sync
//   - Vec<T>: Send + Sync where T: Send + Sync
//   - HashMap<K, V>: Send + Sync where K, V: Send + Sync
//   - serde_json::Value: Send + Sync
//   - All enums (ChangeTrigger, etc.): Send + Sync
//
// No manual unsafe impl needed. The compiler verifies all of this.
```

### 12.2 Arc<EventBus> Sharing

The bus is shared via `Clone` (not `Arc`). Because `broadcast::Sender` and `watch::Sender` are internally reference-counted, cloning `HypercolorBus` is cheap -- it increments atomic reference counts, not deep-copy.

```rust
// Every subsystem gets its own clone.
let bus = HypercolorBus::new();

// Render loop
let render_bus = bus.clone();
tokio::spawn(async move {
    render_loop(render_bus).await;
});

// Device manager
let device_bus = bus.clone();
tokio::spawn(async move {
    device_manager(device_bus).await;
});

// WebSocket handler (one per connection)
async fn handle_ws(ws: WebSocket, bus: HypercolorBus) {
    let mut events = bus.subscribe_all();
    let mut frames = bus.subscribe_frames();
    // ...
}

// Axum route handler passes the bus via State
let app = Router::new()
    .route("/api/v1/ws", get(ws_handler))
    .with_state(bus.clone());
```

If external code needs a shared reference without cloning (e.g., for a trait object), `Arc<HypercolorBus>` works:

```rust
let shared_bus: Arc<HypercolorBus> = Arc::new(bus);

// Pass to trait objects
let plugin: Box<dyn DevicePlugin> = Box::new(WledPlugin::new(shared_bus.clone()));
```

### 12.3 No Blocking in Event Handlers

**Rule: never hold a lock, block on I/O, or perform CPU-intensive work inside an event handler on the bus.**

The `broadcast` channel's `send()` blocks all receivers momentarily while the message is written. If a receiver's handler blocks, it delays all other receivers on the same channel. In practice, `tokio::broadcast` uses a lock-free ring buffer, so `send()` is O(1) and non-blocking. But receiver processing should still be fast.

**Correct pattern -- spawn blocking work:**

```rust
let mut events = bus.subscribe_all();
loop {
    match events.recv().await {
        Ok(event) => {
            // Fast: update local state
            update_ui_state(&event);

            // If the event triggers expensive work, spawn it
            if let HypercolorEvent::ProfileLoaded { .. } = &event.event {
                tokio::spawn(async move {
                    reload_profile_resources().await;
                });
            }
        }
        Err(RecvError::Lagged(n)) => { /* ... */ }
        Err(RecvError::Closed) => break,
    }
}
```

**Incorrect pattern -- blocking inside handler:**

```rust
// DON'T DO THIS -- blocks the receiver and delays event processing
match events.recv().await {
    Ok(event) => {
        // Blocking file I/O inside an event handler
        std::fs::write("event.log", serde_json::to_string(&event)?)?;
        // Slow network call
        reqwest::get("http://ha.local/webhook").await?;
    }
}
```

### 12.4 Graceful Shutdown Sequence

```
1. Daemon receives SIGTERM / SIGINT / user shutdown command
2. Emit ShutdownRequested event on the broadcast channel
3. Wait `grace_period_secs` for subscribers to persist state
4. Emit DaemonShutdown event on the broadcast channel
5. Wait 100ms for subscribers to receive the shutdown event
6. Drop the watch senders (frame, spectrum, active_effect, fps)
   -- subscribers see channel closed
7. Drop the broadcast sender -- subscribers see channel closed
8. Close IPC listeners (Unix socket, named pipe, TCP)
9. Close WebSocket connections (send Close frame)
10. Clean up socket file / named pipe
11. Exit
```

### 12.5 Bandwidth Budget

Typical bandwidth for a full-featured TUI session (1,356 LEDs, audio active):

| Stream | Size/msg | Rate | Bandwidth |
|--------|----------|------|-----------|
| LED frames | ~2.7 KB | 30 fps | 81 KB/s |
| Spectrum (64 bins) | 287 B | 30 fps | 8.6 KB/s |
| Events | ~200 B | ~10/s | 2 KB/s |
| **Total** | | | **~92 KB/s** |

Typical bandwidth for a web UI session with spatial editor open:

| Stream | Size/msg | Rate | Bandwidth |
|--------|----------|------|-----------|
| LED frames | ~2.7 KB | 30 fps | 81 KB/s |
| Spectrum (64 bins) | 287 B | 30 fps | 8.6 KB/s |
| Canvas (RGB) | 192 KB | 15 fps | 2,880 KB/s |
| Events | ~200 B | ~10/s | 2 KB/s |
| **Total** | | | **~2.97 MB/s** |

Canvas is the expensive channel. Only subscribe when the spatial editor is open.

### 12.6 Testing Considerations

**Unit tests:** Instantiate `HypercolorBus` with real tokio channels. Publish events, assert subscribers receive them. Verify `watch` latest-value semantics by publishing multiple frames and confirming the subscriber only sees the last. Test `EventFilter` matching logic exhaustively.

**Integration tests:** Spawn a real daemon in test mode, connect via Unix socket, exercise the full JSON-RPC protocol. Verify subscription lifecycle, binary frame decoding, and reconnection behavior.

**Fuzz testing:** Fuzz the binary frame decoder (`FrameData::from_binary`, `SpectrumData::from_binary`) with arbitrary byte sequences. Fuzz the JSON-RPC dispatcher with malformed JSON. The daemon must never crash on malformed input.

**Load testing:** Open 50 simultaneous WebSocket connections, all subscribed to frames + spectrum + events. Verify the daemon maintains 60fps render rate and all subscribers receive data within latency targets.
