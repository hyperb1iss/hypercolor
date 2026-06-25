+++
title = "WebSocket protocol"
description = "The /api/v1/ws protocol: subprotocol token, JSON client/server messages, all 13 subscription channels, and the binary frame wire format."
weight = 30
template = "page.html"
+++

One WebSocket carries the daemon's entire live surface. Open `/api/v1/ws`, read
the `hello` snapshot, subscribe to the channels you want, and the daemon streams
exactly those — JSON for control, events, metrics, and sensors; binary frames for
LED color, audio spectrum, and canvas previews. The web UI, the TUI, and any
custom client all speak this one protocol, so you never poll and never juggle a
second HTTP connection.

{% callout(type="info") %}
The wire contract here is generated from the daemon source on `main`. The JSON
message shapes come from `crates/hypercolor-daemon/src/api/ws/protocol.rs`; the
binary frame layouts are owned by `hypercolor-leptos-ext::ws` and round-trip
tested against the daemon encoders in `daemon/src/api/ws/tests.rs`. When the code
and this page disagree, the code wins — file an issue.
{% end %}

## Connect

```
ws://localhost:9420/api/v1/ws
```

The daemon advertises the subprotocol token `hypercolor-v1` during the upgrade
(`Sec-WebSocket-Protocol`). Browsers and `websocat` negotiate this automatically;
if you build the handshake by hand, request that token.

When API-key auth is configured, the WebSocket upgrade is the one route that
accepts the key as a `?token=` query parameter (because browser `WebSocket`
constructors cannot set an `Authorization` header):

```
ws://localhost:9420/api/v1/ws?token=<your-api-key>
```

Native clients that control the request headers should instead send
`Authorization: Bearer <your-api-key>`, the same scheme the REST API uses. See
[auth and security](@/api/_index.md) for the dual-key model. Loopback clients on
the default unsecured daemon need no key at all.

{% callout(type="warning") %}
Browser origin is enforced on the upgrade. Requests with no `Origin` header
(native and CLI clients) and loopback origins are always allowed. A non-loopback
browser origin is rejected unless it appears in the daemon's `web.cors_origins`
allowlist and auth is enabled. A blocked upgrade returns `403 Forbidden` before
the socket opens.
{% end %}

### Quick test

Using [websocat](https://github.com/vi/websocat):

```bash
websocat ws://localhost:9420/api/v1/ws
```

You will immediately see one `hello` message, then a quiet socket until you
subscribe.

## Protocol shape

The server is a subscription multiplexer. A single socket carries two
interleaved streams:

- **JSON text messages** for the handshake, control commands, discrete events,
  metrics, and sensor snapshots. Every JSON message is tagged with a `type`
  field.
- **Binary frames** for high-rate data: LED color frames, audio spectrum, and
  the various canvas/zone preview surfaces. Each binary frame starts with a tag
  byte that identifies its channel.

JSON and binary messages may arrive in any order. Branch on the message kind
before parsing:

```javascript
ws.onmessage = (event) => {
  if (event.data instanceof ArrayBuffer) {
    const tag = new DataView(event.data).getUint8(0);
    handleBinaryFrame(tag, event.data);
  } else {
    handleJsonMessage(JSON.parse(event.data));
  }
};
```

The daemon never pushes data for a channel you have not subscribed to, with one
exception: the `events` channel is active on every connection from the moment it
opens.

{% mermaid() %}
sequenceDiagram
  participant C as Client
  participant D as Daemon
  C->>D: GET /api/v1/ws (Upgrade, hypercolor-v1)
  D-->>C: hello (state snapshot + capabilities)
  Note over C,D: events channel already live
  C->>D: subscribe { channels: [frames, spectrum] }
  D-->>C: subscribed (echoed config)
  loop while subscribed
    D-->>C: binary frame 0x01 (LED colors)
    D-->>C: binary frame 0x02 (spectrum)
    D-->>C: event (effect_started, ...)
  end
  C->>D: unsubscribe { channels: [frames] }
  D-->>C: unsubscribed (remaining channels)
{% end %}

## The hello handshake

On connect the daemon sends exactly one `hello`, carrying a current-state
snapshot, its identity, the full capability list, and the channels already
subscribed.

```json
{
  "type": "hello",
  "version": "1.0",
  "server": {
    "instance_id": "a1b2c3d4-...",
    "instance_name": "hypercolor",
    "version": "0.1.0"
  },
  "state": {
    "running": true,
    "paused": false,
    "brightness": 100,
    "fps": { "target": 60, "actual": 59.8 },
    "effect": { "id": "borealis", "name": "Borealis" },
    "scene": { "id": "late-night", "name": "Late Night", "snapshot_locked": false },
    "profile": null,
    "layout": null,
    "device_count": 3,
    "total_leds": 432
  },
  "capabilities": [
    "frames", "spectrum", "events", "frame_events", "canvas",
    "screen_canvas", "screen_zones", "web_viewport_canvas", "zone_preview",
    "metrics", "device_metrics", "sensors", "display_preview",
    "commands", "canvas_format_jpeg"
  ],
  "subscriptions": ["events"]
}
```

`version` is the protocol version (`"1.0"`), distinct from the
`server.version` daemon build string. `capabilities` lists all 13 channel names
plus two feature flags (`commands`, `canvas_format_jpeg`). `subscriptions` shows
what is already live — only `events` by default.

The `effect`, `scene`, `profile`, and `layout` fields are nullable: each is
`null` when nothing is active. The `scene` reference additionally carries
`snapshot_locked`, which is true while a scene blocks runtime mutation.

## Channels

Thirteen subscription channels carry the daemon's live surface. Subscribe by
name; the daemon relays each channel's frames until you unsubscribe or the
socket closes.

| Channel | Wire | Description |
| --- | --- | --- |
| `events` | JSON | Discrete bus events. Active by default. |
| `frame_events` | JSON | High-rate per-frame render-timing events (the `frame_rendered` stream the `events` channel suppresses). |
| `frames` | Binary | Per-zone LED color frames. |
| `spectrum` | Binary | Audio spectrum, levels, and beat data. |
| `canvas` | Binary | The composed RGBA render canvas. |
| `screen_canvas` | Binary | Screen-capture canvas. Control-tier only. |
| `screen_zones` | Binary | Smoothed per-sector ambilight grid from screen capture. Control-tier only. |
| `web_viewport_canvas` | Binary | Servo web-viewport canvas (HTML effect output). |
| `zone_preview` | Binary | Per-zone preview frames, addressed by scene and zone. |
| `metrics` | JSON | Periodic render-performance snapshot. |
| `device_metrics` | JSON | Periodic per-device output telemetry. |
| `sensors` | JSON | Periodic host sensor snapshot (system telemetry). |
| `display_preview` | Binary | Per-display JPEG preview for LCD/display devices. |

{% callout(type="warning") %}
`screen_canvas` and `screen_zones` expose live screen-capture pixels, so they
require a control-tier subscription. On a secured daemon, subscribing without a
control key returns an `error` with code `forbidden` and `required_tier:
"control"`. On the default unsecured loopback daemon there is no key to provide
and the subscription succeeds.
{% end %}

## Client messages

Clients send JSON messages tagged with `type`. Five message types are accepted:
`subscribe`, `unsubscribe`, `command`, `zone_layout_preview`, and
`zone_layout_preview_clear`.

### subscribe

Subscribe to one or more channels. An optional `config` patch tunes per-channel
parameters in the same message; only the channels you name are touched, and the
rest keep their current settings.

```json
{
  "type": "subscribe",
  "channels": ["frames", "metrics"],
  "config": {
    "frames": { "fps": 30, "format": "binary", "zones": ["all"] },
    "metrics": { "interval_ms": 1000 }
  }
}
```

The daemon acknowledges with `subscribed`, echoing the resolved config for every
currently subscribed channel that exposes one:

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

### unsubscribe

```json
{ "type": "unsubscribe", "channels": ["frames"] }
```

The daemon replies with `unsubscribed`, listing the channels it dropped and the
channels still active:

```json
{
  "type": "unsubscribed",
  "channels": ["frames"],
  "remaining": ["events", "metrics"]
}
```

### command

Run any REST call over the open socket. This lets one connection both receive
streams and issue mutations without a second HTTP request. The `id` is
client-assigned and echoed back so concurrent commands can be correlated.

```json
{
  "type": "command",
  "id": "cmd-001",
  "method": "POST",
  "path": "/api/v1/effects/borealis/apply",
  "body": { "controls": { "speed": 7 } }
}
```

The daemon answers with a `response` message carrying the HTTP status and the
result body:

```json
{
  "type": "response",
  "id": "cmd-001",
  "status": 200,
  "data": { "effect": { "id": "borealis", "name": "Borealis" } }
}
```

On error, `status` reflects the HTTP status and `error` is populated instead of
`data`. Write commands over a read-only key are rejected the same way the REST
layer rejects them. See [the REST reference](@/api/rest.md) for the full route
surface you can drive this way.

### zone_layout_preview

Stage one scene zone's spatial layout for live Studio drag interactions. The
override is scoped to this WebSocket session, affects preview rendering only, and
clears automatically when the socket closes. It preserves the zone's output
roster and applies the payload for placement edits only. This message requires a
control-tier key.

```json
{
  "type": "zone_layout_preview",
  "scene_id": "default",
  "zone_id": "0197495b-3513-72f6-9c42-a278a8b6d90f",
  "layout": {
    "id": "default-zone-layout-preview",
    "name": "Default zone",
    "canvas_width": 640,
    "canvas_height": 480,
    "zones": [],
    "default_sampling_mode": { "type": "bilinear" },
    "default_edge_behavior": "clamp",
    "spaces": null,
    "version": 1
  }
}
```

`scene_id` accepts a scene UUID or the literal `default`; `zone_id` must be a
zone UUID. The preview layout must contain exactly the selected zone's outputs —
no more, no fewer. For the difference between scenes (whole-rig configs) and
zones (canvas partitions), see [the Studio docs](@/studio/_index.md).

### zone_layout_preview_clear

Clear one staged zone-layout override before the connection closes. Also
control-tier.

```json
{
  "type": "zone_layout_preview_clear",
  "scene_id": "default",
  "zone_id": "0197495b-3513-72f6-9c42-a278a8b6d90f"
}
```

## Server messages

Beyond the `hello`, `subscribed`, `unsubscribed`, and `response` messages
already shown, the daemon emits the following JSON messages on subscribed
channels.

### event

Relayed from the internal event bus on the `events` channel. Event names are
snake_case derivations of the internal enum variants. High-rate
`frame_rendered` events are excluded here; subscribe to `frame_events` when you
want raw per-frame timing.

```json
{
  "type": "event",
  "event": "effect_started",
  "timestamp": "2026-06-24T18:03:11.482Z",
  "data": { "effect_id": "borealis", "effect_name": "Borealis" }
}
```

Common event names include `effect_started`, `effect_stopped`,
`effect_control_changed`, `device_connected`, `device_disconnected`,
`active_scene_changed`, `beat_detected`, and `profile_applied`.

### metrics

Periodic render-performance snapshot on the `metrics` channel, sent at the
configured `interval_ms` (default 1000 ms). The `data` object is large — it
includes FPS, frame-time percentiles, per-stage timing, pacing jitter, effect
and Servo health counters, render-surface pool gauges, preview demand, memory,
device output, and WebSocket statistics. A representative subset:

```json
{
  "type": "metrics",
  "timestamp": "2026-06-24T18:03:11.482Z",
  "data": {
    "fps": { "target": 60, "ceiling": 60, "actual": 59.8, "dropped": 0 },
    "frame_time": { "avg_ms": 4.2, "p95_ms": 5.1, "p99_ms": 6.0, "max_ms": 8.3 },
    "devices": { "connected": 3, "total_leds": 432, "output_errors": 0 }
  }
}
```

{% callout(type="tip") %}
Treat `metrics.data` as an open, additive object: read the fields you need by
name and ignore the rest. The daemon adds counters over time (Servo render
stages, GPU import slots, SparkleFlinger finalize stats), so a client that
hard-asserts on the full key set will break on upgrade.
{% end %}

### device_metrics

Periodic per-device output telemetry on the `device_metrics` channel, also
governed by `interval_ms`.

### sensors

Latest host sensor snapshot on the `sensors` channel — system telemetry the TUI
and dashboard surface. The `data` object is a `SystemSnapshot`.

```json
{
  "type": "sensors",
  "timestamp": "2026-06-24T18:03:11.482Z",
  "data": { "...": "system snapshot fields" }
}
```

### backpressure

Sent when the daemon is dropping binary frames for a consumer that cannot keep
up. The outbound binary queue is bounded, so a slow client is throttled by
dropped frames rather than unbounded daemon memory growth.

```json
{
  "type": "backpressure",
  "dropped_frames": 12,
  "channel": "frames",
  "recommendation": "Reduce fps or zone count to keep up with the stream",
  "suggested_fps": 15
}
```

React by lowering the channel's `fps` with a fresh `subscribe` config patch.

### error

A protocol-level error: malformed JSON, an unknown channel, an invalid config
value, or a forbidden control-tier subscription.

```json
{
  "type": "error",
  "code": "unsupported_channel",
  "message": "Channel 'bogus' is not supported by this server",
  "details": { "channel": "bogus" }
}
```

Error codes you may see: `invalid_request` (bad JSON or empty channel list),
`invalid_config` (out-of-range or invalid config value, with `details.field`
and `details.reason`), `unsupported_channel`, and `forbidden` (a control-tier
subscription or mutation attempted without a control key).

## Channel configuration

Each configurable channel carries parameters that control throughput and format.
Send them in the `config` field of a `subscribe` message. Out-of-range values
are rejected with an `invalid_config` error and the channel is left unchanged.

### frames config

| Field | Type | Default | Range / values |
| --- | --- | --- | --- |
| `fps` | integer | `30` | 1..=60 |
| `format` | string | `"binary"` | `"binary"` or `"json"` |
| `zones` | array of string | `["all"]` | zone IDs, or `["all"]`; must not be empty |

### spectrum config

| Field | Type | Default | Range / values |
| --- | --- | --- | --- |
| `fps` | integer | `30` | 1..=60 |
| `bins` | integer | `64` | one of 8, 16, 32, 64, 128 |

### canvas / screen_canvas / web_viewport_canvas / zone_preview config

These four preview channels share the same config shape:

| Field | Type | Default | Range / values |
| --- | --- | --- | --- |
| `fps` | integer | `15` | 1..=60 |
| `format` | string | `"rgb"` | `"rgb"`, `"rgba"`, or `"jpeg"` |
| `width` | integer | `0` | 0..=4096 (0 = daemon canvas width) |
| `height` | integer | `0` | 0..=4096 (0 = daemon canvas height) |

The canvas dimensions default to the daemon's configured render size, which is
640×480 unless `daemon.canvas_width`/`daemon.canvas_height` change it — never
assume a fixed size.

### metrics / device_metrics config

| Field | Type | Default | Range / values |
| --- | --- | --- | --- |
| `interval_ms` | integer | `1000` | 100..=10000 |

### display_preview config

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `device_id` | string or null | none | Target display device ID. Send `null` to clear the target and stop the relay. |
| `fps` | integer | `15` | 1..=30 |

`display_preview` uses a tri-state for `device_id`: omit the key to leave the
current target untouched, send a string to follow that display, or send `null`
to detach. Frames on this channel are always JPEG.

`screen_zones` has no client-tunable config — it relays the daemon's
screen-capture grid as produced.

## Binary frame formats

Every binary frame opens with a tag byte at offset 0 identifying its channel.
The preview, spectrum, frames, and zone frames carry that single tag and then
their own header; the optional CinderRPC frames (tags `0x80`/`0x81`) add a
second schema-version byte. All integers are little-endian.

| Tag | Channel | Header length |
| --- | --- | --- |
| `0x01` | `frames` | 10 bytes |
| `0x02` | `spectrum` | 27 bytes |
| `0x03` | `canvas` | 14 bytes |
| `0x05` | `screen_canvas` | 14 bytes |
| `0x06` | `web_viewport_canvas` | 14 bytes |
| `0x07` | `display_preview` | 14 bytes |
| `0x08` | `zone_preview` | 46 bytes |
| `0x09` | `screen_zones` | 19 bytes |
| `0x80` | RPC request | 2-byte prefix |
| `0x81` | RPC response | 2-byte prefix |

{% callout(type="info") %}
`0x04` is intentionally unused in the current channel set. The preview-family
tags (`0x03`/`0x05`/`0x06`/`0x07`) share one header layout, distinguished only by
the leading tag.
{% end %}

### frames (0x01)

Per-zone LED colors. Header is 10 bytes, then one block per zone.

```
Byte(s)  Field
0        tag = 0x01
1-4      frame_number (u32 LE)
5-8      timestamp_ms (u32 LE)
9        zone_count (u8, max 255)

For each zone (repeated zone_count times):
  2      zone_id length (u16 LE)
  N      zone_id UTF-8 bytes (N = zone_id length)
  2      led_count (u16 LE)
  3×M    RGB bytes (M = led_count; R, G, B per LED)
```

When `format` is `"json"`, the same data arrives as a structured JSON message
instead. Binary is strongly preferred for throughput.

### spectrum (0x02)

Audio spectrum, summary levels, and beat detection. Header is 27 bytes, then the
per-bin magnitudes. BPM is not in this frame — read it from the `metrics`
channel.

```
Byte(s)  Field
0        tag = 0x02
1-4      timestamp_ms (u32 LE)
5        bin_count (u8)
6-9      level (f32 LE, overall level 0.0-1.0)
10-13    bass (f32 LE)
14-17    mid (f32 LE)
18-21    treble (f32 LE)
22       beat (u8, 0 or 1)
23-26    beat_confidence (f32 LE)
27..     bins (bin_count × f32 LE)
```

### canvas / screen_canvas / web_viewport_canvas / display_preview (0x03 / 0x05 / 0x06 / 0x07)

The preview family shares one 14-byte header. `display_preview` (`0x07`) is
always JPEG; the others honor the `format` you subscribed with.

```
Byte(s)  Field
0        tag (0x03 / 0x05 / 0x06 / 0x07)
1-4      frame_number (u32 LE)
5-8      timestamp_ms (u32 LE)
9-10     width (u16 LE)
11-12    height (u16 LE)
13       format: 0 = RGB, 1 = RGBA, 2 = JPEG
14..     payload bytes
```

For raw formats the payload is `width × height × bytes_per_pixel` (3 for RGB, 4
for RGBA). JPEG payloads have no fixed size and run to the end of the frame.

### zone_preview (0x08)

A preview addressed to a specific scene and zone. Header is 46 bytes: it inserts
two 16-byte UUIDs between the timestamp and the dimensions.

```
Byte(s)  Field
0        tag = 0x08
1-4      frame_number (u32 LE)
5-8      timestamp_ms (u32 LE)
9-24     scene_id (16 raw UUID bytes)
25-40    zone_id (16 raw UUID bytes)
41-42    width (u16 LE)
43-44    height (u16 LE)
45       format: 0 = RGB, 1 = RGBA, 2 = JPEG
46..     payload bytes
```

### screen_zones (0x09)

The smoothed ambilight grid extracted from screen capture — the same per-sector
colors screen-reactive effects sample. Header is 19 bytes, then a row-major RGB
grid of `grid_cols × grid_rows × 3` bytes.

```
Byte(s)  Field
0        tag = 0x09
1-4      frame_number (u32 LE)
5-8      timestamp_ms (u32 LE)
9-10     source_width (u16 LE)
11-12    source_height (u16 LE)
13       grid_cols (u8)
14       grid_rows (u8)
15-18    letterbox bars (u8 each: top, bottom, left, right, in grid units)
19..     RGB payload (grid_cols × grid_rows × 3 bytes, row-major)
```

### RPC frames (0x80 / 0x81)

The CinderRPC request/response frames are the one binary type that uses the
two-byte `BinaryFrameSchema` prefix — byte 0 is the tag, byte 1 is the schema
version (currently `1`) — before the body. They are part of the shared
`hypercolor-leptos-ext::ws` wire vocabulary and are not part of the standard
subscription channel set; most clients never need them. Request bodies carry a
`u64` id, a length-prefixed method string, and an opaque payload; responses
carry the matching `u64` id, a `u16` status code, and a payload.

## Worked example

A minimal browser client that connects, subscribes to LED frames after the
handshake, and dispatches binary frames by tag:

```javascript
const ws = new WebSocket("ws://localhost:9420/api/v1/ws", "hypercolor-v1");
ws.binaryType = "arraybuffer";

ws.onmessage = (event) => {
  if (event.data instanceof ArrayBuffer) {
    const view = new DataView(event.data);
    const tag = view.getUint8(0);
    // 0x01 frames, 0x02 spectrum, 0x03/0x05/0x06/0x07 previews,
    // 0x08 zone_preview, 0x09 screen_zones
    if (tag === 0x01) parseFramePayload(view);
    return;
  }

  const msg = JSON.parse(event.data);
  if (msg.type === "hello") {
    ws.send(JSON.stringify({
      type: "subscribe",
      channels: ["frames"],
      config: { frames: { fps: 30, format: "binary", zones: ["all"] } },
    }));
  }
};
```

## Connection lifecycle and reconnect

- The daemon sends `hello` immediately on connect. No polling is needed.
- The `events` channel is live from the start; everything else requires an
  explicit `subscribe`.
- The daemon keeps the socket alive with a ping every 30 seconds and closes a
  client that fails to pong within 10 seconds. Respond to pings (browsers and
  most libraries do this automatically).
- There is no protocol-level auto-reconnect. If the daemon restarts, the socket
  closes and you must reconnect, re-read the `hello`, and re-subscribe.
- Multiple concurrent connections are supported; each has its own independent
  subscription set.

A resilient client wraps the socket in a reconnect loop with backoff, and on
every (re)connect waits for `hello` before re-issuing its subscriptions:

```javascript
function connect() {
  const ws = new WebSocket("ws://localhost:9420/api/v1/ws", "hypercolor-v1");
  ws.binaryType = "arraybuffer";

  ws.onmessage = (event) => {
    if (typeof event.data === "string") {
      const msg = JSON.parse(event.data);
      if (msg.type === "hello") resubscribe(ws);
    }
  };

  ws.onclose = () => setTimeout(connect, backoff.next());
  return ws;
}
```

For the request/response REST surface those `command` messages mirror, and the
shared envelope they return, see [the REST API reference](@/api/rest.md). For
driving the daemon from AI agents, see [the agents docs](@/agents/_index.md).
