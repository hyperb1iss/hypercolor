+++
title = "Event bus"
description = "HypercolorBus: broadcast vs watch vs canvas-watch, and the watch-vs-broadcast rule."
weight = 20
+++

`HypercolorBus` is the nervous system that connects the render loop, device backends, audio processor, WebSocket server, and every other subsystem. It is `Send + Sync`, cheaply cloneable via `Arc`, and entirely lock-free at the channel level. Every component that needs to observe or publish state goes through the bus — nothing reaches across subsystem boundaries directly.

Three distinct communication patterns live on the same bus, each chosen for a different data shape:

| Pattern | Tokio primitive | Who gets it | Drops on full? |
|---|---|---|---|
| Discrete events | `broadcast` (capacity 256) | Every active subscriber | Yes — slow subscriber falls behind |
| High-frequency data streams | `watch` (latest-value) | Each subscriber gets the newest value | Stale frames are skipped automatically |
| Per-zone display canvases | `watch` per `ZoneId` | Display consumers keyed by zone | Same as watch |

The governing rule: **events are broadcast; data streams are watch.** Using broadcast for frame colors or spectrum data would cause slow subscribers to lag and consume memory proportional to the event backlog. Watch discards stale values by design, which is exactly what high-frequency streaming data needs.

## Broadcast: discrete events

`HypercolorEvent` is the full taxonomy of state changes the daemon can emit. The enum has ten categories — `Device`, `Effect`, `Scene`, `Audio`, `System`, `Asset`, `Automation`, `Layout`, `Input`, `Integration` — and four priority levels (`Low`, `Normal`, `High`, `Critical`).

```rust
// Subscribe to all events
let mut rx = bus.subscribe_all();
let event = rx.recv().await?;

// Subscribe to a filtered subset
use hypercolor_core::bus::EventFilter;
use hypercolor_types::event::EventCategory;

let filter = EventFilter::new().categories(vec![EventCategory::Device, EventCategory::Effect]);
let mut rx = bus.subscribe_filtered(filter);
```

`FilteredEventReceiver` wraps a broadcast receiver and silently consumes non-matching events. A `RecvError::Lagged(n)` return means the subscriber fell behind by `n` events; the correct response is to request a fresh state snapshot from the REST API rather than trying to reconstruct history from a partial queue.

The broadcast channel is sized at 256 events (`EVENT_CHANNEL_CAPACITY`). At a steady-state rate of 10–30 events per second that gives 8–25 seconds of headroom for a stalled subscriber before events start dropping. Burst scenarios such as 8 devices connecting simultaneously during discovery stay well within that window.

### Event timestamps

The bus adds two timestamps at publish time so producers do not need clocks:

- `timestamp` — wall-clock ISO 8601 in milliseconds (`EventTimestamp`, formatted as `YYYY-MM-DDTHH:MM:SS.mmmZ`)
- `mono_ms` — monotonic milliseconds since bus creation, useful for correlating events to rendered frames

### Event priorities

Priority is declared in the event type itself, not by the publisher:

- **Critical** — `DaemonShutdown`, `ShutdownRequested`, fatal errors. These must be handled before the bus closes.
- **High** — `DeviceConnected`, `DeviceDisconnected`, `DeviceError`, layer and effect failures. Delivered with strong guarantees.
- **Normal** — Most scene, profile, config, and effect events.
- **Low** — `BeatDetected`, `AudioLevelUpdate`, `FrameRendered`, input events, discovery completions. These are informational; missing one or two is not a problem.

## Watch: high-frequency data streams

Four watch senders handle continuous data. Each returns the latest value immediately on subscribe, then notifies on every update. A slow subscriber simply sees the current value when it gets around to checking — no queue accumulates.

| Sender | Type | Published by | Rate |
|---|---|---|---|
| `frame` | `FrameData` | Render loop | Per-frame (10–60 Hz) |
| `spectrum` | `SpectrumData` | Audio processor | ~30–60 Hz |
| `canvas` | `CanvasFrame` | Render loop (preview) | Per-frame |
| `scene_canvas` | `CanvasFrame` | Render loop (authoritative) | Per-frame |

`FrameData` carries per-zone LED color arrays for all active zones. It is what device backends read — each backend subscribes to `frame_receiver()` and sends colors to hardware at its own cadence.

`SpectrumData` carries the reduced audio summary: overall level, bass/mid/treble bands, beat state, BPM estimate, and 200 normalized FFT bins. This is distinct from the full `AudioData` struct consumed inside effect renderers; `SpectrumData` is the bus-level snapshot intended for external consumers like the WebSocket API and TUI meters.

```rust
// Consuming frame data in a device backend
let mut rx = bus.frame_receiver();
loop {
    rx.changed().await?;
    let frame = rx.borrow_and_update();
    // frame.zones is the current LED color state
}

// Consuming spectrum data
let mut rx = bus.spectrum_receiver();
loop {
    rx.changed().await?;
    let spectrum = rx.borrow_and_update();
    // spectrum.bass, spectrum.beat, spectrum.bins, etc.
}
```

{% callout(type="warning") %}
Never subscribe to `frame` or `spectrum` data via the broadcast channel — those paths do not exist. Both are watch senders. Using broadcast for these data shapes would queue every frame for every slow subscriber; watch gives latest-value semantics with zero queue buildup.
{% end %}

## Canvas streams

The bus carries several canvas watch senders for different consumers:

| Sender | Purpose |
|---|---|
| `canvas` | Render-loop preview snapshot — WebSocket `canvas` channel |
| `scene_canvas` | Authoritative full-scene surface for non-preview consumers |
| `screen_canvas` | Screen-capture source preview |
| `web_viewport_canvas` | High-resolution web viewport preview |
| `zone_preview` | Batch of per-zone `ZonePreviewFrame` values for Studio |

`CanvasFrame` stores RGBA bytes regardless of the downstream transport format. Width and height are included per-frame because the canvas is configurable (`daemon.canvas_width` / `daemon.canvas_height`; defaults 640×480).

Per-render-group canvases are keyed by `ZoneId` and created on demand:

```rust
// Publisher side (render loop)
let sender = bus.group_canvas_sender(zone_id);
sender.send(DisplayGroupFrame::from_surface(surface))?;

// Consumer side (display output)
let mut rx = bus.group_canvas_receiver(zone_id);
rx.changed().await?;
let frame = rx.borrow_and_update();
```

When zones are removed, the daemon calls `retain_group_canvases` to drop stale senders rather than letting them accumulate.

`DisplayGroupFrame` is an enum that carries either `Canvas(CanvasFrame)` (RGBA) or `Yuv420(DisplayYuv420Frame)` (planar YUV420 for display output devices). Consumers pattern-match on the variant.

## Screen zones

`ScreenZonesFrame` is a separate watch channel (`screen_zones`) that carries the ambilight zone grid extracted from screen capture: smoothed, color-tuned per-sector RGB values in row-major order, plus capture metadata (source resolution, grid dimensions, detected letterbox bars). Effects that react to screen content consume this rather than the raw screen canvas.

## Bus lifecycle

`HypercolorBus` is constructed once at daemon startup and shared via clone (each clone shares the same underlying channel state via `Arc`). The bus starts a monotonic clock at construction time for `mono_ms` timestamps; all per-frame timing data can be correlated back to that origin.

```rust
let bus = HypercolorBus::new();
// Share by cloning — cheap, no allocation
let bus2 = bus.clone();
```

The bus has no shutdown method; it drops when the last clone drops. Broadcast subscribers receive `RecvError::Closed` and watch receivers return stale values after the sender drops, which signals that the daemon is shutting down.

## WebSocket wire format

The daemon's WebSocket API exposes bus data to external consumers. The binary frame protocol is defined in `hypercolor-leptos-ext::ws` (protocol `hypercolor-v1`) and is the single source of truth for encoding; both the web UI and the TUI decode using the same types. See [@/api/websocket.md](@/api/websocket.md) for the channel subscription model, frame streaming, and payload layouts.

## Render pipeline integration

The full pipeline from input to device output:

```
InputManager::sample_all()         → Collect audio, screen, keyboard data
build_frame_scene_snapshot()       → Capture active scene and live control state
SparkleFlinger::compose_frame()    → Blend producers into one RGBA canvas (640×480 default)
SpatialEngine::sample()            → Map canvas pixels to LED positions → ZoneColors
BackendManager::write_frame()      → Group by device, queue async sends
event_bus senders                  → frame_sender (FrameData watch), canvas_sender (CanvasFrame watch), publish(FrameRendered) broadcast
```

The `FrameRendered` broadcast event carries `FrameTiming` with per-stage microsecond costs (`producer_us`, `composition_us`, `render_us`, `sample_us`, `push_us`, `total_us`, `budget_us`). The WebSocket and TUI subscribe to this for live performance metrics.

See [@/architecture/render-pipeline.md](@/architecture/render-pipeline.md) for the full render loop and FPS adaptation policy.
