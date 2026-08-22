+++
title = "WebSocket protocol"
description = "The /api/v1/ws protocol: subprotocol token, JSON client/server messages, all 15 subscription topics, and the binary frame wire format."
weight = 30
template = "page.html"
+++

One WebSocket carries the daemon's entire live surface. Open `/api/v1/ws`, read
the `hello` snapshot, subscribe to the topics you want, and the daemon streams
exactly those: JSON for control, events, metrics, and sensors; binary frames for
LED color, audio spectrum, and canvas previews. The web UI, the TUI, and any
custom client all speak this one protocol, so you never poll and never juggle a
second HTTP connection.

{% <callout type="info"> %}
The wire contract here is generated from the daemon source on `main`. The JSON
message shapes come from `crates/hypercolor-daemon/src/api/ws/protocol.rs`; the
binary frame layouts are owned by `hypercolor-leptos-ext::ws` and round-trip
tested against the daemon encoders in `daemon/src/api/ws/tests.rs`. When the code
and this page disagree, the code wins; file an issue.
{% </callout> %}

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
[auth and security](@/api/auth-and-security.md) for the dual-key model.
Loopback clients on the default unsecured daemon need no key at all.

{% <callout type="warning"> %}
Browser origin is enforced on the upgrade. Requests with no `Origin` header
(native and CLI clients) and loopback origins are always allowed. A non-loopback
browser origin is rejected unless it appears in the daemon's `web.cors_origins`
allowlist and auth is enabled. A blocked upgrade returns `403 Forbidden` before
the socket opens.
{% </callout> %}

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
  byte that identifies its topic.

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

The daemon never pushes data for a subscription you do not hold, with one
exception: the `events` topic is active on every connection from the moment it
opens.

{% <mermaid> %}
sequenceDiagram
  participant C as Client
  participant D as Daemon
  C->>D: GET /api/v1/ws (Upgrade, hypercolor-v1)
  D-->>C: hello (state snapshot + capabilities)
  Note over C,D: events topic already live
  C->>D: subscribe { topics, preview_transport }
  D-->>C: subscribed (live subscriptions + effective transport)
  loop while subscribed
    D-->>C: binary frame 0x01 (LED colors)
    D-->>C: binary frame 0x02 (spectrum)
    D-->>C: event (effect_started, ...)
  end
  C->>D: unsubscribe { topics: [frames] }
  D-->>C: unsubscribed (remaining subscriptions)
{% </mermaid> %}

## The hello handshake

On connect the daemon sends exactly one `hello`, carrying a current-state
snapshot, its identity, the full capability list, and the subscriptions already
live.

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
    "fps": { "target": 60, "capacity": 60.0, "delivered": 59.8 },
    "scene": { "id": "late-night", "name": "Late Night", "snapshot_locked": false },
    "layout": null,
    "device_count": 3,
    "total_leds": 432
  },
  "capabilities": [
    "frames", "spectrum", "events", "frame_events", "canvas",
    "screen_canvas", "screen_zones", "web_viewport_canvas", "zone_preview",
    "metrics", "device_metrics", "sensors", "display_preview",
    "interactive_preview", "input_events",
    "commands", "canvas_format_jpeg", "interactive_previews",
    "wide_preview_frames", "preview_chunking",
    "preview_transport_v2:decoded=536870912,encoded=536936448,connection=1073872896,reassembly=8388608,tombstones=4194304,sender=8388608,cursors=8388608,idle_ms=5000,message=1048576"
  ],
  "subscriptions": [{ "topic": "events" }]
}
```

`version` is the protocol version (`"1.0"`), distinct from the
`server.version` daemon build string. `capabilities` lists all 15 topic names
plus feature flags such as `commands`, `canvas_format_jpeg`,
`interactive_previews`, `wide_preview_frames`, and `preview_chunking`.
`subscriptions` uses the same entry shape the acknowledgments do, so a client
reads its live set the same way everywhere.

The daemon advertises one `preview_transport_v2` capability. The capability
describes receiver memory contracts for decoded and encoded publications,
connection retention, reassembly state, tombstones, sender state, cursor state,
idle expiry, and individual WebSocket messages. Chunk admission derives from
the encoded byte budget instead of a separate object-count ceiling.

A client sends its own capability string in the optional `preview_transport`
field of its first `subscribe`; the daemon applies the field-by-field minimum
before activating preview topics and returns the effective capability in
`subscribed`. A client that omits the field uses the daemon's advertised
defaults. Renegotiation after a preview publication is active is rejected
instead of changing limits underneath in-flight state.

The advertised message budget must be at least 184 bytes so every bounded
stream identity can still carry a one-byte publication fragment. Under V1 the
encoded budget cannot exceed the bytes representable by the advertised message
and chunk counts, and the connection budget must hold two maximum encoded
publications so latest-value replacement can retain the active writer until its
cancellation is observed while admitting the replacement. V2 tracks that
retention through its explicit sender and cursor byte budgets, so it requires
only that the connection budget hold one maximum encoded publication.

Neither version changes a single byte on the binary wire. Chunked publications
use the same `0x0F` envelope at schema `1` in both, so a decoder never has to
ask which version negotiated the stream it is reading.

`subscriptions` shows
what is already live, only `events` by default.

The `scene` and `layout` fields are nullable: each is `null` when nothing is
active. The `scene` reference additionally carries
`snapshot_locked`, which is true while a scene blocks runtime mutation.

## Topics

Fifteen subscription topics carry the daemon's live surface. Subscribe by name;
the daemon relays each subscription's frames until you unsubscribe or the socket
closes.

Two topics are **keyed**: a subscription to one of them names both the topic and
the key it follows, and one connection holds as many subscriptions to that topic
as it names distinct keys. Every other topic is unkeyed and holds at most one
subscription per connection.

| Topic | Key | Wire | Description |
| --- | --- | --- | --- |
| `events` | — | JSON | Discrete bus events. Active by default. |
| `frame_events` | — | JSON | High-rate per-frame render-timing events (the `frame_rendered` stream the `events` topic suppresses). |
| `frames` | — | Binary | Per-zone LED color frames. |
| `spectrum` | — | Binary | Audio spectrum, levels, and beat data. |
| `canvas` | — | Binary | The composed RGBA render canvas. |
| `screen_canvas` | — | Binary | Screen-capture canvas. Control-tier only. |
| `screen_zones` | — | Binary | Smoothed per-sector ambilight grid from screen capture. Control-tier only. |
| `web_viewport_canvas` | — | Binary | Servo web-viewport canvas (HTML effect output). |
| `zone_preview` | — | Binary | Per-zone preview frames, addressed by scene and zone. |
| `metrics` | — | JSON | Periodic render-performance snapshot. |
| `device_metrics` | — | JSON | Periodic per-device output telemetry. |
| `sensors` | — | JSON | Periodic host sensor snapshot (system telemetry). |
| `display_preview` | device id | Binary | One display device's JPEG output preview. |
| `interactive_preview` | preview id | Binary | An interactive scene preview lane the subscription itself opens. Control-tier only. |
| `input_events` | — | JSON | Timed keyboard and pointer events from the input pipeline. Control-tier only. |

{% <callout type="warning"> %}
`screen_canvas`, `screen_zones`, `interactive_preview`, and `input_events`
expose live screen-capture pixels, host input activity, or a render lane of
their own, so they require a control-tier subscription. On a secured daemon,
subscribing without a control key returns an `error` with code `forbidden` and
`required_tier: "control"`. On the default unsecured loopback daemon there is no
key to provide and the subscription succeeds.
{% </callout> %}

## Client messages

Clients send JSON messages tagged with `type`. Subscription, command, layout
preview, and connection-scoped interactive preview messages are accepted. The
canonical inventory lives in `protocol/websocket-v1.json`.

### subscribe

`topics` is an array of subscriptions. Each entry names a `topic`, a `key` when
that topic is keyed, and an optional `config` patch for that one subscription.
Config travels with its selector, so a patch can only ever reach a subscription
the same message establishes. Only the subscriptions you name are touched; the
rest keep their current settings.

Preview-capable clients should also send a `preview_transport` capability string
on the first subscription so both peers enforce the same physical budgets. Send
`preview_transport_v2` unless you only implement the V1 count-based limits.

```json
{
  "type": "subscribe",
  "preview_transport": "preview_transport_v2:decoded=536870912,encoded=536936448,connection=1073872896,reassembly=8388608,tombstones=4194304,sender=8388608,cursors=8388608,idle_ms=5000,message=1048576",
  "topics": [
    { "topic": "frames", "config": { "fps": 30, "zones": ["all"] } },
    { "topic": "metrics", "config": { "fps": 1.0 } },
    { "topic": "display_preview", "key": "3f2504e0-4f89-11d3-9a0c-0305e82c3301", "config": { "fps": 15 } }
  ]
}
```

The whole request is one transaction. Any rejection returns an `error` and
leaves every subscription exactly as it was, so a message that names four
subscriptions and mis-configures the fourth changes nothing.

The daemon acknowledges with `subscribed`, reporting **every** live subscription
on the connection rather than only the ones this message named. A topic that
takes no config reports none:

```json
{
  "type": "subscribed",
  "preview_transport": "preview_transport_v2:decoded=536870912,encoded=536936448,connection=1073872896,reassembly=8388608,tombstones=4194304,sender=8388608,cursors=8388608,idle_ms=5000,message=1048576",
  "topics": [
    { "topic": "frames", "config": { "fps": 30, "zones": ["all"] } },
    { "topic": "events" },
    { "topic": "metrics", "config": { "fps": 1.0 } },
    { "topic": "display_preview", "key": "3f2504e0-4f89-11d3-9a0c-0305e82c3301", "config": { "fps": 15 } }
  ]
}
```

An `interactive_preview` entry also carries a `publication_id`: the identity of
the render lane the subscription opened. Fence that preview's binary frames
against it so a previous incarnation's stragglers are discarded.

### unsubscribe

Each entry is a selector: a topic, plus its key when the topic is keyed.
Retiring one key of a keyed topic leaves its other keys live.

```json
{
  "type": "unsubscribe",
  "topics": [
    { "topic": "frames" },
    { "topic": "display_preview", "key": "3f2504e0-4f89-11d3-9a0c-0305e82c3301" }
  ]
}
```

The daemon replies with `unsubscribed`, carrying the same whole-connection
snapshot of what remains:

```json
{
  "type": "unsubscribed",
  "topics": [
    { "topic": "events" },
    { "topic": "metrics", "config": { "fps": 1.0 } }
  ]
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
  "body": { "controls": { "speed": { "float": 7.0 } } }
}
```

The daemon answers with a `response` message carrying the HTTP status and the
result body:

```json
{
  "type": "response",
  "id": "cmd-001",
  "status": 200,
  "data": {
    "zone": {
      "id": "018f5f8f-20f8-7e69-a6a0-5c0fc23e7481",
      "name": "Desk",
      "role": "primary",
      "enabled": true,
      "brightness": 1.0,
      "color": null,
      "display_target": null,
      "members": [],
      "layout": null,
      "layers": [
        {
          "id": "019b2eb9-4083-7e5a-b6f1-82a2e735b798",
          "source": { "type": "effect", "effect_id": "borealis" }
        }
      ]
    },
    "transition": { "type": "cut" },
    "output": { "applied": true }
  }
}
```

Successful commands unwrap the REST response envelope, so `data` is the
canonical apply result itself: `{ zone, transition, output }`.

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

`zone_id` must be a zone UUID in the **active** scene. Previews only ever apply
to what is rendering, so there is no scene to select: the daemon owns which one
that is. The preview layout must contain exactly the selected zone's outputs, no
more, no fewer. For the difference between scenes (whole-rig configs) and
zones (canvas partitions), see [the Studio docs](@/studio/_index.md).

### zone_layout_preview_clear

Clear one staged zone-layout override before the connection closes. Also
control-tier.

```json
{
  "type": "zone_layout_preview_clear",
  "zone_id": "0197495b-3513-72f6-9c42-a278a8b6d90f"
}
```

### Interactive previews

Check for the `interactive_previews` capability before using this flow. An
interactive preview is a keyed subscription: subscribing opens its render lane
and unsubscribing closes it, with the preview id as the key.

```json
{
  "type": "subscribe",
  "topics": [{
    "topic": "interactive_preview",
    "key": "main",
    "config": {
      "target": "active_scene",
      "fps": 30,
      "width": 640,
      "height": 480,
      "format": "jpeg"
    }
  }]
}
```

The acknowledgment carries that subscription's `publication_id`, and the daemon
then streams binary `0x0A` frames naming the same key. Subscribing again with a
different config resizes or retargets the lane in place, keeping its
publication. Unlike the passive preview canvases, an interactive lane has no
server-picked size: zero `width` or `height` is refused rather than resolved.

Pointer and key batches name the live preview:

```json
{
  "type": "input_inject",
  "preview_id": "main",
  "events": [
    { "kind": "move", "nx": 0.5, "ny": 0.25 },
    { "kind": "key", "key": "a", "state": "pressed" }
  ]
}
```

The daemon also accepts `interactive_preview_claim_authoritative` and
`interactive_preview_release_authoritative` for explicitly routing that browser
source to authoritative device output. Those messages and the subscription
itself require a control-tier connection. Closing the socket releases previews,
injected held state, and authoritative claims.

Source-less `input_inject` messages are not accepted: every batch names the
preview it drives.

## Server messages

Beyond the `hello`, `subscribed`, `unsubscribed`, and `response` messages
already shown, addressed preview commands receive `input_injected`,
`interactive_preview_authoritative_claimed`, or
`interactive_preview_authoritative_released`. The daemon also emits the
following JSON messages on subscribed topics.

### event

Relayed from the internal event bus on the `events` topic. Event names are
snake_case derivations of the internal enum variants. High-rate
`frame_rendered` events are excluded here; subscribe to `frame_events` when you
want raw per-frame timing.

```json
{
  "type": "event",
  "event": "effect_started",
  "timestamp": "2026-06-24T18:03:11.482Z",
  "data": {
    "effect": { "id": "borealis", "name": "Borealis", "engine": "native" },
    "trigger": "api",
    "previous": null,
    "transition": null,
    "zone_id": "018f5f8f-20f8-7e69-a6a0-5c0fc23e7481",
    "zone_name": "Desk"
  }
}
```

Common event names include `effect_started`, `effect_stopped`,
`effect_control_changed`, `zone_changed`, `device_connected`,
`device_disconnected`, `active_scene_changed`, `scene_library_changed`, and
`beat_detected`.

Zone-addressed events use `zone_id`. Lifecycle events also carry `zone_name`;
`zone_changed` carries `scene_id`, `role`, and `kind`; and
`scene_settings_changed` carries the live scene document's single `revision`.
Saved scenes on disk still
store the concept as `groups`, while the public wire uses zone vocabulary.

### metrics

Periodic render-performance snapshot on the `metrics` topic, sent at the
configured `fps` cadence (default 1 fps). Fractional values support slower
cadences, such as `0.5` fps for one publication every two seconds. The `data`
object is large: it
includes FPS, frame-time percentiles, per-stage timing, pacing jitter, effect
and Servo health counters, render-surface pool gauges, preview demand, memory,
device output, and WebSocket statistics. A representative subset:

```json
{
  "type": "metrics",
  "timestamp": "2026-06-24T18:03:11.482Z",
  "data": {
    "fps": { "target": 60, "ceiling": 60, "capacity": 59.8, "delivered": 59.2, "dropped": 0 },
    "frame_time": { "avg_ms": 4.2, "p95_ms": 5.1, "p99_ms": 6.0, "max_ms": 8.3 },
    "devices": { "connected": 3, "total_leds": 432, "output_errors": 0 }
  }
}
```

{% <callout type="tip"> %}
Treat `metrics.data` as an open, additive object: read the fields you need by
name and ignore the rest. The daemon adds counters over time (Servo render
stages, GPU import slots, SparkleFlinger finalize stats), so a client that
hard-asserts on the full key set will break on upgrade.
{% </callout> %}

### device_metrics

Periodic per-device output telemetry on the `device_metrics` topic, also
governed by `fps`.

### sensors

Latest host sensor snapshot on the `sensors` topic: the system telemetry the
TUI and dashboard surface. The `data` object is a `SystemSnapshot`.

```json
{
  "type": "sensors",
  "timestamp": "2026-06-24T18:03:11.482Z",
  "data": { "...": "system snapshot fields" }
}
```

### backpressure

Every topic declares how it behaves when a subscriber cannot keep up, and the
manifest states the class per topic:

| Class | Meaning | Topics |
| --- | --- | --- |
| `lossless` | The connection queue uses an awaited send. If the upstream broadcast bus itself lags, the daemon emits `resync_required` instead of hiding the discontinuity. | `events`, `frame_events`, `input_events` |
| `latest_wins` | Only the newest value matters, so a slow reader skips what it missed. | `canvas`, `screen_canvas`, `screen_zones`, `web_viewport_canvas`, `zone_preview`, `display_preview`, `interactive_preview`, `sensors` |
| `drop_with_notice` | A message that will not fit is dropped and the subscriber is told, so it can reduce its own demand. | `frames`, `spectrum`, `metrics`, `device_metrics` |

Sent when the daemon drops data for a `drop_with_notice` consumer that cannot
keep up. Preview publications use a different policy: one latest
publication per stream under a connection byte budget. A newer publication
replaces queued work for the same stream. A different stream waits for retained
bytes to leave the socket before its latest publication is admitted. Neither
path grows daemon memory without a bound or strands a terminal latest value.

The notice names the topic it dropped, and a keyed topic also names the key.

```json
{
  "type": "backpressure",
  "dropped_frames": 12,
  "topic": "frames",
  "recommendation": "reduce_fps",
  "suggested_fps": 15
}
```

Clients can patch the subscription to match the bandwidth they intend to consume.
All four topics use `reduce_fps` with `suggested_fps`. Telemetry suggestions may
be fractional.
Daemon metrics expose preview queue bytes plus queued, replaced, rejected,
sent-publication, and sent-chunk counters for diagnosing the actual bottleneck.

### Continuity: events are not replayed

The events channel carries live changes only. A client that loses the socket
misses every event during the gap, and the daemon does not replay them on
reconnect. Subscribe first, wait for the initial `subscribed` acknowledgment,
then refetch every resource you mirror. Repeat that barrier on reconnect. The
daemon installs the default events receiver before it snapshots `hello`, so
changes after the handshake snapshot are already queued. Refetch again whenever
a `resync_required` event arrives, which the daemon sends
when a subscriber falls far enough behind that events were dropped on a socket
that is still open.

This is why the handshake is deliberately thin. It reports how the daemon is
running, not what is rendering: the live tree is multi-zone and multi-layer, so
read [`GET /api/v1/scene`](@/api/rest.md) for content and follow the events
channel for changes.

### error

A protocol-level error: malformed JSON, an unknown topic, a missing or invalid
key, an invalid config value, or a forbidden control-tier subscription.

```json
{
  "type": "error",
  "code": "validation_error",
  "message": "Invalid configuration for config.frames.fps: expected 1..=60",
  "details": { "field": "config.frames.fps", "reason": "expected 1..=60" }
}
```

Socket refusals use the same codes the REST envelope does, so there is no second
table to learn. Error codes you may see: `malformed_request` (bad JSON, an empty
`topics` array,
an unknown topic name, a keyed topic named without a key or an unkeyed one named
with one, or the same subscription named twice in one message), `validation_error`
(an invalid config patch, with `details.field` and `details.reason`), `forbidden`
(a control-tier subscription or mutation attempted without a control key),
`conflict` (a preview ownership or live-state conflict), and
`service_unavailable` (a requested runtime capability is unavailable).

## Topic configuration

Each configurable topic carries parameters that control throughput and format.
Send them in the `config` field of that subscription's entry in a `subscribe`
message. A rejected patch fails the whole request with a `validation_error`
error, and every subscription named in it is left exactly as it was.

Each patch is validated by the topic that owns it, so four shapes are refused
rather than ignored: a value outside the documented range, a field the topic
does not define, a patch sent for a topic that takes no config (`events`,
`frame_events`, `sensors`, `input_events`), and the same
subscription named twice in one `topics` array. A `null` config on a
configurable topic means "leave this subscription alone".

### frames config

| Field | Type | Default | Range / values |
| --- | --- | --- | --- |
| `fps` | integer | `30` | 1..=60 |
| `zones` | array of string | `["all"]` | zone IDs, or `["all"]`; must not be empty |

### spectrum config

| Field | Type | Default | Range / values |
| --- | --- | --- | --- |
| `fps` | integer | `30` | 1..=60 |
| `bins` | integer | `64` | one of 8, 16, 32, 64, 128 |

### canvas / screen_canvas / web_viewport_canvas / zone_preview config

These four preview topics share the same config shape:

| Field | Type | Default | Range / values |
| --- | --- | --- | --- |
| `fps` | integer | `15` | 1..=60 |
| `format` | string | `"rgb"` | `"rgb"`, `"rgba"`, or `"jpeg"` |
| `width` | integer | `0` | unsigned 32-bit; 0 selects the source width or preserves aspect ratio |
| `height` | integer | `0` | unsigned 32-bit; 0 selects the source height or preserves aspect ratio |

Raw RGB and RGBA previews retain the full unsigned 32-bit axis vocabulary,
subject only to the advertised decoded-byte budget. Standard JPEG stores each
axis in 16 bits, so JPEG requests are rejected explicitly when either resolved
axis exceeds 65,535 pixels. The daemon never silently resizes an over-limit
request.

The canvas dimensions default to the daemon's configured render size, which is
640×480 unless `daemon.canvas_width`/`daemon.canvas_height` change it, so never
assume a fixed size. If both dimensions are zero, the daemon publishes the source
size. If exactly one is zero, it derives that axis from the source aspect ratio.
Admission is based on checked pixel and byte counts, not an arbitrary axis ceiling.
The maximum accepted preview surface is currently 512 MiB at four bytes per pixel.

### metrics / device_metrics config

| Field | Type | Default | Range / values |
| --- | --- | --- | --- |
| `fps` | number | `1.0` | 0.1..=10.0 |

### screen_zones config

| Field | Type | Default | Range / values |
| --- | --- | --- | --- |
| `fps` | integer | `15` | 1..=60 |

### display_preview config

Keyed by device id, so the display a subscription follows is its key rather than
a config field.

| Field | Type | Default | Range / values |
| --- | --- | --- | --- |
| `fps` | integer | `15` | 1..=30 |

Frames on this topic are always JPEG, and each one names the device it came
from, so a connection following several displays routes them without guessing.

### interactive_preview config

Keyed by the client's own preview id. Subscribing opens a render lane at exactly
the requested shape, so zero dimensions are refused rather than resolved.

| Field | Type | Default | Range / values |
| --- | --- | --- | --- |
| `target` | string | `"active_scene"` | `"active_scene"` |
| `fps` | integer | `30` | 1..=60 |
| `width` | integer | `640` | unsigned 32-bit, non-zero |
| `height` | integer | `480` | unsigned 32-bit, non-zero |
| `format` | string | `"jpeg"` | `"rgb"`, `"rgba"`, or `"jpeg"` |

`screen_zones` applies this cadence per connection even when another preview
subscriber drives the shared capture source at a higher rate.

## Binary frame formats

Every binary frame opens with a tag byte at offset 0. Direct preview, spectrum,
frames, and zone messages continue with their type-specific header. The preview
transport control frames (`0x0F` and `0x10`) add a schema byte at offset 1. All
integers are little-endian.

| Tag | Topic | Header length |
| --- | --- | --- |
| `0x01` | `frames` | 11 bytes |
| `0x02` | `spectrum` | 27 bytes |
| `0x03` | `canvas` | 14 bytes |
| `0x05` | `screen_canvas` | 14 bytes |
| `0x06` | `web_viewport_canvas` | 14 bytes |
| `0x07` | `display_preview` | 15 bytes + device id |
| `0x08` | `zone_preview` | 46 bytes |
| `0x09` | `screen_zones` | 19 bytes |
| `0x0A` | addressed interactive preview | 15 bytes + preview id |
| `0x0B` | wide preview family | 19 bytes |
| `0x0C` | wide zone preview | 50 bytes |
| `0x0D` | wide addressed interactive preview | 19 bytes + preview id |
| `0x0E` | wide screen zones | 23 bytes |
| `0x0F` | chunked preview envelope | 55 bytes + stream identity |
| `0x10` | preview publication cancellation | 14 bytes + stream identity |
| `0x11` | extended screen zones | 41 bytes |
| `0x12` | wide display preview | 19 bytes + device id |

{% <callout type="info"> %}
`0x04` is intentionally unused in the current topic set. The passive
preview-canvas tags (`0x03`/`0x05`/`0x06`) share one header layout,
distinguished only by the leading tag. Display preview left that family when it
became keyed: its frames carry the device id, so they use the same
identity-prefixed layout as interactive previews.
{% </callout> %}

### frames (0x01)

Per-zone LED colors. Header is 11 bytes, then one block per zone.

```
Byte(s)  Field
0        tag = 0x01
1-4      frame_number (u32 LE)
5-8      timestamp_ms (u32 LE)
9-10     zone_count (u16 LE)

For each zone (repeated zone_count times):
  2      zone_id length (u16 LE)
  N      zone_id UTF-8 bytes (N = zone_id length)
  2      led_count (u16 LE)
  3×M    RGB bytes (M = led_count; R, G, B per LED)
```

Frames are binary. The JSON encoding this topic used to offer is deleted: it had
no consumers, and it routed frames down the text queue, which opted the topic
that most needs a backpressure notice out of receiving one.

### spectrum (0x02)

Audio spectrum, summary levels, and beat detection. Header is 27 bytes, then the
per-bin magnitudes. BPM is not in this frame; read it from the `metrics`
topic.

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

### canvas / screen_canvas / web_viewport_canvas (0x03 / 0x05 / 0x06)

The passive preview canvases share one 14-byte header and remain byte-exact for
dimensions that fit `u16`. Each honors the `format` you subscribed with. Raw
dimensions above 65,535 use the additive wide layout documented below; JPEG's
intrinsic 16-bit axis limit is validated before encoding.

```
Byte(s)  Field
0        tag (0x03 / 0x05 / 0x06)
1-4      frame_number (u32 LE)
5-8      timestamp_ms (u32 LE)
9-10     width (u16 LE)
11-12    height (u16 LE)
13       format: 0 = RGB, 1 = RGBA, 2 = JPEG
14..     payload bytes
```

For raw formats the payload is `width × height × bytes_per_pixel` (3 for RGB, 4
for RGBA). JPEG payloads have no fixed size and run to the end of the frame.

### display_preview (0x07)

One display device's output frame, always JPEG. The device id is in the header
because the topic is keyed by device: a connection following three displays
receives three interleaved streams and routes them by name rather than by
guessing from resolution. The fixed prefix is 15 bytes, followed by the UTF-8
device id and the image payload.

```
Byte(s)  Field
0        tag = 0x07
1        device_id length (u8)
2-5      frame_number (u32 LE)
6-9      timestamp_ms (u32 LE)
10-11    width (u16 LE)
12-13    height (u16 LE)
14       format: 2 = JPEG
15..N    device_id UTF-8 bytes
N..      payload bytes
```

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

The smoothed ambilight grid extracted from screen capture, the same per-sector
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

### Addressed interactive preview (0x0A)

This frame is pushed by a live `interactive_preview` subscription. The fixed
prefix is 15 bytes, followed by the UTF-8 preview id and image payload; the
layout matches `display_preview`'s, with a preview id in place of a device id.

```
Byte(s)  Field
0        tag = 0x0A
1        preview_id length (u8)
2-5      frame_number (u32 LE)
6-9      timestamp_ms (u32 LE)
10-11    width (u16 LE)
12-13    height (u16 LE)
14       format: 0 = RGB, 1 = RGBA, 2 = JPEG
15..N    preview_id UTF-8 bytes
N..      payload bytes
```

Route frames by `preview_id`, not arrival order. A connection may own more
than one preview, and closing then reopening an id creates a new publication
lifetime.

### Wide preview dimensions (0x0B / 0x0C / 0x0D / 0x0E / 0x12)

Wide tags replace each `u16` dimension with `u32` while preserving the remaining
field order. `0x0B` adds the original passive channel tag at byte 1, followed by
`frame_number`, `timestamp_ms`, `width`, `height`, `format`, and payload. The
wide zone, interactive, display, and screen-zone layouts otherwise mirror their
narrow forms. Clients should decode both layouts; servers keep sending narrow
bytes whenever both axes fit `u16`.

### Chunked preview publication (0x0F)

One WebSocket message is limited to 1 MiB. A larger preview publication is split
into ordered `0x0F` messages instead of being resized or truncated. The 55-byte
fixed envelope carries schema `1`, stream kind and channel, pixel format, stream
identity length, publication id, frame metadata, total encoded length, chunk
offset, chunk index, and chunk count. The stream identity follows the fixed
header, then that chunk's payload bytes.

Reassembly is keyed by stream identity and publication id. Chunks must begin at
index and offset zero, remain contiguous, and keep identical metadata. Clients
must bound partial state by bytes and stream count, discard it on reconnect, and
validate the completed publication against the envelope metadata before display.

The transport also bounds each connection's active partials and reclaimable
high-water tombstones. A partial that receives no chunk for the negotiated idle
interval expires on a wall-clock deadline, even when no more messages arrive.

### Preview publication cancellation (0x10)

When a queued or partially sent publication is superseded, evicted,
unsubscribed, or explicitly closed, the server sends a schema `1` cancellation
for that exact stream and publication id. The 14-byte fixed header contains the
tag, schema, stream kind, channel tag, identity length, and `u64` publication
id. The stream identity follows the header.

Clients release any partial publication at or below that id while retaining the
stream high-water mark. A cancellation never removes a newer publication for
the same stream. Recent high-water tombstones reject delayed stale chunks;
older tombstones are reclaimed within the advertised bound.

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
    // 0x01 frames, 0x02 spectrum, 0x03/0x05/0x06 preview canvases,
    // 0x07 display, 0x08 zone, 0x09 screen zones, 0x0A interactive,
    // 0x0B-0x0E and 0x12 wide, 0x0F preview chunks,
    // 0x10 publication cancellation
    if (tag === 0x01) parseFramePayload(view);
    return;
  }

  const msg = JSON.parse(event.data);
  if (msg.type === "hello") {
    ws.send(JSON.stringify({
      type: "subscribe",
      topics: [
        { topic: "frames", config: { fps: 30, zones: ["all"] } },
      ],
    }));
  }
};
```

## Connection lifecycle and reconnect

- The daemon sends `hello` immediately on connect. No polling is needed.
- The `events` topic is live from the start; everything else requires an
  explicit `subscribe`.
- The daemon keeps the socket alive with a ping every 30 seconds and closes a
  client that fails to pong within 10 seconds. Respond to pings (browsers and
  most libraries do this automatically).
- There is no protocol-level auto-reconnect. If the daemon restarts, the socket
  closes and you must reconnect, re-read the `hello`, and re-subscribe.
- Multiple concurrent connections are supported; each has its own independent
  subscription set.

A resilient client wraps the socket in a reconnect loop with backoff. On every
connection it waits for `hello`, re-issues its subscriptions, then refetches
authoritative REST state after the first `subscribed` acknowledgment:

```javascript
function connect() {
  const ws = new WebSocket("ws://localhost:9420/api/v1/ws", "hypercolor-v1");
  ws.binaryType = "arraybuffer";
  let awaitingBootstrap = true;

  ws.onmessage = (event) => {
    if (typeof event.data === "string") {
      const msg = JSON.parse(event.data);
      if (msg.type === "hello") resubscribe(ws);
      if (msg.type === "subscribed" && awaitingBootstrap) {
        awaitingBootstrap = false;
        refetchMirroredResources();
      }
    }
  };

  ws.onclose = () => setTimeout(connect, backoff.next());
  return ws;
}
```

For the request/response REST surface those `command` messages mirror, and the
shared envelope they return, see [the REST API reference](@/api/rest.md). For
driving the daemon from AI agents, see [the agents docs](@/agents/_index.md).
