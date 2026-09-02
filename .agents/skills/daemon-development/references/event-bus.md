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
    zone_id: Some(zone_id),
    zone_name: Some(zone_name),
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
let rx = bus.subscribe_filtered(
    EventFilter::new().categories(vec![EventCategory::Effect]),
);
```

`EventFilter` is a builder over two optional fields: `new()`, then `categories(Vec<EventCategory>)` and `min_priority(EventPriority)`, combining with AND. There is no `category(..)` singular constructor.

A struct expression must name every field even where `#[serde(default)]` is present: `serde(default)` governs deserialization only, so omitting `zone_id` or `zone_name` above is an E0063, not a default. `zone_id` and `zone_name` are `None` only for publishers with no zone context, such as session restore.

**Use for**: Discrete state change notifications (effect started, device connected, config changed). Multiple consumers need every event.

### 2. Watch (tokio::sync::watch)

**Latest-value only** -- consumers see the most recent value, not a queue. Each stream is a `WatchLane<T>`, a typed latest-value channel that also counts publications and revisions. The bus exposes lane accessors and receiver accessors; there are no public fields and no `*_sender()` accessors for the unkeyed lanes.

```rust
// Publishing through the lane (returns &WatchLane<T>)
bus.frame_lane().send_replace(frame_data);
bus.spectrum_lane().send_replace(spectrum_data);
bus.canvas_lane().send_replace(canvas_frame);

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

`WatchLane` offers `subscribe()`, `borrow()`, `send()` (fails when no receiver exists), `send_replace()` (always replaces), and the `_weighted` forms that count an explicit number of logical payloads for telemetry.

Eight unkeyed lanes exist, each with a matching `_lane()`, `_receiver()`, and `_receiver_count()` accessor: `frame`, `spectrum`, `canvas`, `scene_canvas`, `screen_canvas`, `screen_zones`, `web_viewport_canvas`, `zone_preview`. The one keyed stream is per-zone display output, reached through `bus.zone_canvas_sender(zone_id)` and `bus.zone_canvas_receiver(zone_id)`, which return real `watch::Sender` / `watch::Receiver` handles rather than a lane.

**Use for**: High-frequency data (frames at 30-60 FPS, audio spectrum, canvas snapshots). Consumers only need latest -- no buffering.

### Note: No MPSC on the Bus

The `HypercolorBus` itself only provides broadcast and watch lanes. API mutations do not send commands to the render thread over a channel: they commit through a domain service, which enqueues a `SceneTransaction` on `AppState::scene_transactions` (a `SceneTransactionQueue`, `src/scene_transactions.rs`). The render thread drains that queue at a frame boundary in `render_thread::frame_executor::service_scene_transactions`. There is no `RenderCommand` type.

## Event Taxonomy

Events are `HypercolorEvent` variants grouped by `EventCategory` with `EventPriority` levels.

| Event                              | Category | Where it is published                                     |
| ---------------------------------- | -------- | --------------------------------------------------------- |
| `EffectStarted`                    | Effect   | `domain/scene.rs`, queued on the scene mutation           |
| `EffectStopped`                    | Effect   | `domain/scene.rs`, on `clear_zone_effect`                 |
| `EffectControlChanged`             | Effect   | `domain/scene.rs`, on the controls patch                  |
| `EffectRegistryUpdated`            | Effect   | `domain/effect.rs`, from the rescan report                |
| `DeviceDiscovered`                 | Device   | `discovery/scan.rs`                                       |
| `DeviceConnected`                  | Device   | `discovery/device_helpers.rs`                             |
| `DeviceDisconnected`               | Device   | `discovery/scan.rs` and `discovery/lifecycle.rs`          |
| `DeviceError`                      | Device   | **nowhere yet**: declared, never constructed              |
| `SceneActivated`                   | Scene    | **nowhere yet**: declared, never constructed              |
| `SceneLibraryChanged`              | Scene    | `domain/scene.rs`, on library create/update/delete        |
| `ConfigChanged`                    | System   | `hypercolor-core` `config/mod.rs`, once per persisted save |
| `FrameRendered`                    | System   | `render_thread/frame_io.rs`                               |
| `FpsChanged`                       | System   | **nowhere yet**: declared, never constructed              |
| `BrightnessChanged`                | System   | `output_power.rs`, alongside `DeviceSettingsChanged`      |
| `BeatDetected`                     | Audio    | **nowhere yet**: declared, never constructed              |
| `AudioLevelUpdate`                 | Audio    | `render_thread/frame_io.rs`                               |
| `LayoutChanged`                    | Layout   | `domain/layout/{workflows,convergence}.rs` and the binding migration publish |
| `DaemonStarted` / `DaemonShutdown` | System   | `startup/lifecycle.rs`                                    |

The five variants marked "nowhere yet" are declared in
`hypercolor-types/src/event.rs`, serialize, and are matched on by relays and
tests, but no production code path constructs them today. Do not wire a
subscriber to one expecting traffic, and do not assume the obvious producer
(the adaptive FPS controller, the spatial engine, the audio processor) is
already publishing.

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
