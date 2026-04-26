# WebSocket Protocol Reference

The daemon WebSocket endpoint at `/api/v1/ws` streams real-time data to the UI.

## Connection Lifecycle

1. **Connect** to `ws://127.0.0.1:9420/api/v1/ws`
2. **Set binary type**: `ws.set_binary_type(BinaryType::Arraybuffer)` — required for canvas frames
3. **Subscribe**: Send JSON `{ "type": "subscribe", "channels": ["events", "metrics"] }`
4. **Receive**: Binary messages (canvas/frame data) and JSON messages (events/metrics/audio)

## Binary Frame Format

Canvas frames arrive as binary `ArrayBuffer` messages. Header byte identifies the type:

| Header | Type         | Content                       |
| ------ | ------------ | ----------------------------- |
| `0x03` | Canvas frame | Frame metadata + pixel buffer |

### Canvas Frame Layout

```
[0]      u8    header (0x03)
[1-4]    u32   frame_number (LE)
[5-8]    u32   timestamp_ms (LE)
[9-10]   u16   width (LE)
[11-12]  u16   height (LE)
[13]     u8    pixel_format (0=RGB, 1=RGBA)
[14..]   bytes pixel data (width * height * bpp)
```

Minimum valid message length: 14 bytes (header + metadata, no pixels). Width and height are **u16** on the wire, upcast to u32 in the `CanvasFrame` struct. Pixel data is a `js_sys::Uint8Array` — direct view of WASM heap, no copy.

## JSON Message Types

```json
// Event (note: subtype key is "event", NOT "event_type")
{ "type": "event", "event": "effect_started", "data": {...} }

// Audio arrives as an event subtype, not a separate message type
{ "type": "event", "event": "audio_level_update", "data": { "level": 0.45, "bass": 0.3, "mid": 0.5, "treble": 0.2, "beat": true } }

// Device events also arrive as event subtypes
{ "type": "event", "event": "device_connected", "data": { "device_id": "..." } }

// Metrics (top-level type, structured data payload)
{ "type": "metrics", "data": { "fps": {...}, "frame_time": {...}, "stages": {...}, ... } }

// Backpressure warning
{ "type": "backpressure", "dropped_frames": 12, "channel": "canvas", "recommendation": "reduce_fps", "suggested_fps": 15 }

// Hello (sent on connect, includes current state)
{ "type": "hello", "state": { "effect": {...}, "fps": { "target": 30, "actual": 29.8 } } }

// Subscribed (confirmation after subscribe request)
{ "type": "subscribed", "config": { "canvas": { "fps": 30 } } }
```

## Reconnection State Machine

```
Connected → (socket close/error) → Disconnected
Disconnected → (wait backoff) → Connecting
Connecting → (success) → Connected
Connecting → (failure) → Disconnected (increment attempt, increase backoff)
```

Backoff: 500ms initial, doubles each attempt, caps at 15s. Resets on successful connection.

On reconnect: re-subscribe to channels, reset FPS smoothing state, clear stale frame data.

## Backpressure Handling

Server-side buffer: 64 events per client. When full, frames are dropped (not queued).

The UI receives `BackpressureNotice` messages suggesting a lower FPS. Canvas preview honors this by reducing its render cap.

## Event Types That Trigger UI Updates

| Event Type            | UI Reaction                                       |
| --------------------- | ------------------------------------------------- |
| `effect_applied`      | Update active effect ID, reload controls          |
| `effect_stopped`      | Clear active state                                |
| `device_connected`    | Refetch device list (if device not already known) |
| `device_disconnected` | Refetch device list (if device was known)         |
| `device_discovered`   | Refetch device list                               |
| `config_changed`      | Reload config resources                           |
| `profile_activated`   | Update active profile, reload state               |

## Closure Lifetime Management

WebSocket callbacks (`onopen`, `onmessage`, `onerror`, `onclose`) use `Closure::<dyn FnMut(...)>::new()` and **must call `.forget()`** to prevent garbage collection:

```rust
let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
    // handle message
});

ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
on_message.forget(); // prevent GC — intentional leak
```

**Important**: Use the `Closure::<dyn FnMut(T)>::new(...)` form (Leptos 0.8 / modern wasm-bindgen), NOT the older `Closure::wrap(Box::new(...) as Box<dyn FnMut(T)>)` pattern.

This is a known WASM pattern. The closure lives for the WebSocket's lifetime. When the socket reconnects, old closures are already forgotten (leaked) — this is acceptable because they're replaced and the old socket is dropped.
