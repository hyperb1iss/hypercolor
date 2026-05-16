+++
title = "WebSocket API"
description = "Real-time channel subscription and bidirectional communication"
weight = 2
template = "page.html"
+++

The WebSocket API provides real-time, bidirectional communication between clients and the Hypercolor daemon. The web UI, TUI, and custom clients use this channel for live state updates, streaming LED frame data, and issuing commands without opening separate HTTP connections.

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

## Protocol Overview

The WebSocket server is a **subscription-channel multiplexer**. Clients choose which data streams they want by subscribing to named channels. The server never pushes data a client has not subscribed to, except for the default `events` channel that is active on every connection.

All messages are JSON with a `type` tag:

```json
{ "type": "message_type", ... }
```

Binary messages are data payloads for the `frames`, `canvas`, `screen_canvas`, `web_viewport_canvas`, and `display_preview` channels. They are never JSON.

## Connect Handshake

On connection the server sends exactly one `hello` message, then begins relaying events. No further server messages arrive until the client subscribes to additional channels.

**Server → Client: `hello`**

```json
{
  "type": "hello",
  "version": "0.1.0",
  "server": {
    "instance_id": "a1b2c3d4-...",
    "instance_name": "hypercolor",
    "version": "0.1.0"
  },
  "state": {
    "running": true,
    "paused": false,
    "brightness": 85,
    "fps": { "target": 60, "actual": 59.8 },
    "effect": { "id": "borealis", "name": "Borealis" },
    "scene": { "id": "late-night", "name": "Late Night", "snapshot_locked": false },
    "profile": { "id": "gaming", "name": "Gaming" },
    "layout": { "id": "default", "name": "Default Layout" },
    "device_count": 3,
    "total_leds": 432
  },
  "capabilities": ["frames", "spectrum", "events", "canvas", "screen_canvas",
                   "web_viewport_canvas", "metrics", "device_metrics",
                   "display_preview", "commands", "canvas_format_jpeg"],
  "subscriptions": ["events"]
}
```

The `subscriptions` field lists which channels are already active. Only `events` is subscribed by default. Every other channel requires an explicit `subscribe` command.

## Channels

| Channel              | Data type | Description                                         |
| -------------------- | --------- | --------------------------------------------------- |
| `events`             | JSON      | HypercolorEvent bus relay — active by default       |
| `frames`             | Binary    | LED color frames per zone                           |
| `spectrum`           | JSON      | Audio spectrum data                                 |
| `canvas`             | Binary    | Rendered RGBA canvas stream                         |
| `screen_canvas`      | Binary    | Screen-capture canvas stream                        |
| `web_viewport_canvas`| Binary    | Web viewport canvas stream                          |
| `metrics`            | JSON      | Render performance metrics (periodic snapshot)      |
| `device_metrics`     | JSON      | Per-device output telemetry (periodic snapshot)     |
| `display_preview`    | Binary    | Per-display JPEG preview frames                     |

## Client Commands

Clients send JSON messages to subscribe, unsubscribe, or issue REST-equivalent commands.

### `subscribe`

Subscribe to one or more channels. An optional `config` patch sets per-channel parameters at the same time.

```json
{
  "type": "subscribe",
  "channels": ["frames", "metrics"],
  "config": {
    "frames": {
      "fps": 30,
      "format": "binary",
      "zones": ["all"]
    },
    "metrics": {
      "interval_ms": 1000
    }
  }
}
```

**Server acknowledges with `subscribed`:**

```json
{
  "type": "subscribed",
  "channels": ["frames", "metrics"],
  "config": {
    "frames": { "fps": 30, "format": "binary", "zones": ["all"] },
    "metrics": { "interval_ms": 1000 }
  }
}
```

### `unsubscribe`

```json
{
  "type": "unsubscribe",
  "channels": ["frames"]
}
```

**Server acknowledges with `unsubscribed`:**

```json
{
  "type": "unsubscribed",
  "channels": ["frames"],
  "remaining": ["events", "metrics"]
}
```

### `command`

Proxy any REST API call over the WebSocket. This lets a single connection both receive streams and issue mutations without opening a separate HTTP connection.

```json
{
  "type": "command",
  "id": "cmd-001",
  "method": "POST",
  "path": "/api/v1/effects/borealis/apply",
  "body": { "controls": { "speed": 7 } }
}
```

**Server responds with `response`:**

```json
{
  "type": "response",
  "id": "cmd-001",
  "status": 200,
  "data": {
    "effect": { "id": "borealis", "name": "Borealis" },
    "applied_controls": { "speed": 7 }
  }
}
```

The `id` field is client-assigned and echoed back so concurrent commands can be correlated. On error, `status` reflects the HTTP status code and `error` is populated instead of `data`.

## Server Messages

### `event`

Relayed from the HypercolorEvent bus. Event names are snake_case derivations of the internal enum variants.

```json
{
  "type": "event",
  "event": "effect_started",
  "timestamp": "2026-05-16T02:14:00Z",
  "data": {
    "effect_id": "borealis",
    "effect_name": "Borealis"
  }
}
```

Common event names: `effect_started`, `effect_stopped`, `effect_control_changed`, `device_connected`, `device_disconnected`, `active_scene_changed`, `beat_detected`, `profile_applied`.

### `metrics`

Periodic render performance snapshot, sent on the `metrics` channel at the configured `interval_ms` (default: 1000 ms). The `data` object includes FPS, frame time percentiles, stage timing breakdowns, memory usage, and WebSocket statistics.

```json
{
  "type": "metrics",
  "timestamp": "2026-05-16T02:14:01Z",
  "data": {
    "fps": { "target": 60, "ceiling": 60, "actual": 59.8, "dropped": 0 },
    "frame_time": { "avg_ms": 4.2, "p95_ms": 5.1, "p99_ms": 6.0, "max_ms": 8.3 },
    "devices": { "connected": 3, "total_leds": 432, "output_errors": 0 }
  }
}
```

### `device_metrics`

Periodic per-device output telemetry snapshot, sent on the `device_metrics` channel.

### `backpressure`

Sent when the server is dropping binary frames for a slow consumer.

```json
{
  "type": "backpressure",
  "dropped_frames": 12,
  "channel": "frames",
  "recommendation": "Reduce fps or zone count to keep up with the stream",
  "suggested_fps": 15
}
```

React to this by reducing the channel's `fps` with an `unsubscribe`/`subscribe` round-trip or by sending a `subscribe` with an updated `config` patch.

### `error`

Protocol-level error, such as an unknown channel name or an invalid config value.

```json
{
  "type": "error",
  "code": "unsupported_channel",
  "message": "Channel 'bogus' is not supported by this server",
  "details": { "channel": "bogus" }
}
```

Error codes: `invalid_request`, `invalid_config`, `unsupported_channel`.

## Channel Configuration

Each channel has configurable parameters that control throughput and format. Send them via the `config` field on a `subscribe` message. Only channels present in `config` are updated; the rest keep their current settings.

### `frames` config

| Field    | Type            | Default    | Range / Values                  |
| -------- | --------------- | ---------- | ------------------------------- |
| `fps`    | integer         | `30`       | 1..=60                          |
| `format` | string          | `"binary"` | `"binary"` or `"json"`          |
| `zones`  | array of string | `["all"]`  | zone IDs or `["all"]`           |

Binary frame format is a flat array of RGB bytes ordered by zone then by LED index. JSON format wraps the same data as a structured object. Binary is strongly preferred for performance.

### `spectrum` config

| Field  | Type    | Default | Range / Values               |
| ------ | ------- | ------- | ---------------------------- |
| `fps`  | integer | `30`    | 1..=60                       |
| `bins` | integer | `64`    | one of: 8, 16, 32, 64, 128  |

### `canvas` / `screen_canvas` / `web_viewport_canvas` config

| Field    | Type    | Default | Range / Values                                |
| -------- | ------- | ------- | --------------------------------------------- |
| `fps`    | integer | `15`    | 1..=60                                        |
| `format` | string  | `"rgb"` | `"rgb"`, `"rgba"`, `"jpeg"`                   |
| `width`  | integer | `0`     | 0..=4096 (0 = use daemon canvas width)        |
| `height` | integer | `0`     | 0..=4096 (0 = use daemon canvas height)       |

### `metrics` / `device_metrics` config

| Field         | Type    | Default | Range / Values   |
| ------------- | ------- | ------- | ---------------- |
| `interval_ms` | integer | `1000`  | 100..=10000      |

### `display_preview` config

| Field       | Type            | Default | Notes                               |
| ----------- | --------------- | ------- | ----------------------------------- |
| `device_id` | string or null  | none    | Target display device ID; `null` to stop |
| `fps`       | integer         | `15`    | 1..=30                              |

Binary payloads on this channel are JPEG-encoded frames for the targeted display.

## Frame Streaming Example 🌊

A minimal JavaScript client that connects, subscribes to frames, and renders them:

```javascript
const ws = new WebSocket("ws://localhost:9420/api/v1/ws");

ws.onmessage = (event) => {
  if (event.data instanceof ArrayBuffer) {
    // Binary frame payload: flat RGB bytes
    renderFrame(new Uint8Array(event.data));
    return;
  }

  const msg = JSON.parse(event.data);

  if (msg.type === "hello") {
    // Subscribe to LED frames at 30fps after the handshake
    ws.send(JSON.stringify({
      type: "subscribe",
      channels: ["frames"],
      config: { frames: { fps: 30, format: "binary", zones: ["all"] } }
    }));
  }
};
```

## Connection Lifecycle

- The server sends `hello` immediately on connection. No polling is needed.
- The `events` channel is subscribed by default. No subscribe message is required to receive events.
- The connection is kept alive with periodic WebSocket pings from the server.
- If the daemon restarts, clients must reconnect. There is no automatic reconnection at the protocol level; implement your own reconnect loop.
- Multiple concurrent WebSocket connections are supported.
- Binary channel data and JSON messages may be interleaved in any order.
