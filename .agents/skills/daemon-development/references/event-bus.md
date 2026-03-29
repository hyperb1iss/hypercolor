# Event Bus Reference

HypercolorBus is the daemon's nervous system — all inter-subsystem communication flows through it.

## Communication Patterns

### 1. Broadcast (tokio::sync::broadcast)

**Capacity**: 256 events. Events are wrapped in `TimestampedEvent` (ISO 8601 timestamp + mono_ms + event payload) before broadcast.

```rust
// Publishing (timestamp added automatically by bus)
bus.publish(HypercolorEvent::EffectStarted {
    effect: EffectRef { id, name, engine },
    trigger: ChangeTrigger::Api,
    previous: None,
    transition: None,
});

// Subscribing (unfiltered -- receives all events)
let mut rx = bus.subscribe_all();
while let Ok(timestamped) = rx.recv().await {
    match &timestamped.event {
        HypercolorEvent::EffectStarted { effect, .. } => { /* ... */ }
        _ => {}
    }
}

// Subscribing (filtered -- only matching events)
let rx = bus.subscribe_filtered(EventFilter::category(EventCategory::Effect));
```

**Use for**: Discrete state change notifications (effect started, device connected, config changed). Multiple consumers need every event.

### 2. Watch (tokio::sync::watch)

**Latest-value only** -- consumers see the most recent value, not a queue. The bus exposes typed sender/receiver accessor methods (no public fields).

```rust
// Publishing via sender accessors (returns &watch::Sender<T>)
bus.frame_sender().send_replace(frame_data);
bus.spectrum_sender().send_replace(spectrum_data);
bus.canvas_sender().send_replace(canvas_frame);

// Subscribing via receiver methods (returns watch::Receiver<T>)
let mut canvas_rx = bus.canvas_receiver();
canvas_rx.changed().await.ok();
let latest = canvas_rx.borrow().clone();

// Or one-shot read from a receiver
let frame_rx = bus.frame_receiver();
let current_frame = frame_rx.borrow().clone();

// Receiver count queries
bus.frame_receiver_count();
bus.spectrum_receiver_count();
bus.canvas_receiver_count();
```

**Use for**: High-frequency data (frames at 30-60 FPS, audio spectrum, canvas snapshots). Consumers only need latest -- no buffering.

### Note: No MPSC on the Bus

The `HypercolorBus` itself only provides broadcast and watch channels. Render commands from API handlers to the render thread use separate `tokio::sync::mpsc` channels created during daemon startup and passed directly to the render loop -- they are not part of the bus API.

## Event Taxonomy

Events are `HypercolorEvent` variants grouped by `EventCategory` with `EventPriority` levels.

| Event | Category | Source |
|-------|----------|--------|
| `EffectStarted` | Effect | API handler, scene activation |
| `EffectStopped` | Effect | Render thread, API stop |
| `EffectControlChanged` | Effect | API handler |
| `EffectRegistryUpdated` | Effect | Rescan |
| `DeviceDiscovered` | Device | Discovery scan |
| `DeviceConnected` | Device | Backend connection |
| `DeviceDisconnected` | Device | Lifecycle manager |
| `DeviceError` | Device | Backend driver |
| `SceneActivated` | Scene | Scene manager |
| `ProfileLoaded` | System | API handler |
| `ConfigChanged` | System | Config API |
| `FrameRendered` | System | Render thread |
| `FpsChanged` | System | Adaptive FPS |
| `BrightnessChanged` | System | Settings API |
| `BeatDetected` | Audio | Audio processor |
| `AudioLevelUpdate` | Audio | Audio processor |
| `LayoutChanged` | Layout | Spatial engine |
| `DaemonStarted` / `DaemonShutdown` | System | Daemon lifecycle |

Priority levels: `Critical` (shutdown, critical errors), `High` (device connect/disconnect), `Normal` (most events), `Low` (frame rendered, beats, webhooks).

## Frame Correlation

Events are wrapped in `TimestampedEvent` which carries both wall-clock and monotonic timestamps:

```rust
pub struct TimestampedEvent {
    pub timestamp: EventTimestamp,  // ISO 8601 wall-clock (serialized as string)
    pub mono_ms: u64,              // monotonic millis since bus creation
    pub event: HypercolorEvent,    // flattened via #[serde(flatten)]
}
```

The `mono_ms` field correlates events with frame timestamps (`FrameData.timestamp_ms`, `CanvasFrame.timestamp_ms`) for matching events to the frame that was rendering when the event occurred -- useful for debugging and metrics display.

## Backpressure

Broadcast channel capacity is 256. When a subscriber falls behind:
- `RecvError::Lagged(n)` indicates `n` missed events
- WebSocket handler logs the lag and catches up
- No memory growth — old events are dropped from the ring buffer

Watch channels have no backpressure concern — they always hold exactly one value.
