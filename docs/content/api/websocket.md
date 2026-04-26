+++
title = "WebSocket API"
description = "Real-time state streaming and bidirectional communication"
weight = 2
template = "page.html"
+++

The WebSocket API provides real-time, bidirectional communication between clients and the Hypercolor daemon. The web UI, TUI, and any custom clients use this channel for live state updates.

## Connection

Connect to the WebSocket endpoint:

```
ws://localhost:9420/api/v1/ws
```

If API key authentication is configured, include the token as a query parameter:

```
ws://localhost:9420/api/v1/ws?token=<your-api-key>
```

### Quick Test

Using [websocat](https://github.com/vi/websocat):

```bash
websocat ws://localhost:9420/api/v1/ws
```

Using JavaScript:

```javascript
const ws = new WebSocket("ws://localhost:9420/api/v1/ws");

ws.onmessage = (event) => {
  const message = JSON.parse(event.data);
  console.log(message.type, message.data);
};
```

## Message Format

All messages are JSON with a consistent envelope:

```json
{
  "type": "event_type",
  "data": { ... }
}
```

### Server-to-Client Events

The daemon pushes these events to all connected WebSocket clients:

**`state`** — Full system state snapshot. Sent on connection and whenever significant state changes occur.

```json
{
  "type": "state",
  "data": {
    "running": true,
    "effect": { "id": "borealis", "name": "Borealis" },
    "controls": { "speed": 5, "intensity": 82 },
    "devices": [...],
    "brightness": 0.8
  }
}
```

**`effect_changed`** — Emitted when the active effect changes.

```json
{
  "type": "effect_changed",
  "data": {
    "effect_id": "borealis",
    "effect_name": "Borealis"
  }
}
```

**`controls_changed`** — Emitted when effect control values are updated.

```json
{
  "type": "controls_changed",
  "data": {
    "speed": 7,
    "palette": "SilkCircuit"
  }
}
```

**`device_connected`** — A new device was discovered and connected.

```json
{
  "type": "device_connected",
  "data": {
    "id": "razer-blackwidow-v4-001",
    "name": "Razer BlackWidow V4",
    "backend": "razer",
    "led_count": 126
  }
}
```

**`device_disconnected`** — A device was disconnected.

```json
{
  "type": "device_disconnected",
  "data": {
    "id": "razer-blackwidow-v4-001"
  }
}
```

**`profile_loaded`** — A profile was applied.

```json
{
  "type": "profile_loaded",
  "data": {
    "profile_id": "gaming",
    "profile_name": "Gaming"
  }
}
```

**`scene_activated`** — A scene was activated.

```json
{
  "type": "scene_activated",
  "data": {
    "scene_id": "late-night",
    "scene_name": "Late Night"
  }
}
```

**`error`** — An error occurred.

```json
{
  "type": "error",
  "data": {
    "code": "device_error",
    "message": "USB device disconnected unexpectedly"
  }
}
```

### Client-to-Server Commands

Clients can send commands through the WebSocket as an alternative to REST calls:

**Apply an effect:**

```json
{
  "type": "apply_effect",
  "data": {
    "effect_id": "borealis",
    "controls": { "speed": 7 }
  }
}
```

**Update controls:**

```json
{
  "type": "set_controls",
  "data": {
    "speed": 3,
    "intensity": 90
  }
}
```

**Stop the effect:**

```json
{
  "type": "stop_effect"
}
```

## Frame Streaming

The WebSocket can stream live LED color data for preview rendering. The web UI uses this to show a real-time visualization of what the LEDs are doing.

Frame data is sent as binary WebSocket messages when frame streaming is active. Each frame contains the current LED colors for all active zones, allowing clients to render a visual preview without needing physical hardware.

{% callout(type="tip", title="Performance") %}
Frame streaming runs at a configurable rate (default: 30fps for previews). The frame rate is independent of the daemon's internal render rate (60fps). This keeps network bandwidth reasonable while still providing smooth previews.
{% end %}

## Connection Lifecycle

- On connect, the server sends an initial `state` message with the full system snapshot
- The connection is kept alive with periodic WebSocket pings
- If the daemon restarts, clients must reconnect — there is no automatic reconnection at the protocol level (clients should implement their own reconnect logic)
- Multiple concurrent WebSocket connections are supported
