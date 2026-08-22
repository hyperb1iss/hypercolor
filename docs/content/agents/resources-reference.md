+++
title = "Resources reference"
description = "The five hypercolor:// MCP resources: ambient read-only context for state, devices, effects, scenes, and audio, with verified JSON shapes."
weight = 30
+++

The MCP server exposes five **resources** under the `hypercolor://` URI scheme. Resources are read-only context an agent can pull at any time without spending a tool call. Where a tool like `get_status` is an explicit action, a resource is ambient knowledge the client subscribes to and re-reads when it needs the current picture. Point an assistant at `hypercolor://state` before it acts and it already knows the daemon is running, which effect is live, and how many devices are lit.

{% <callout type="info"> %}
Resources are part of the same MCP server as the [tools](@/agents/tools-reference.md) and [prompts](@/agents/prompt-templates.md). The server is **off by default**, so enable it first per [MCP setup](@/agents/mcp-setup.md) before any `hypercolor://` URI resolves.
{% </callout> %}

## The five resources

Every resource returns `application/json`. Each carries a priority hint between `0.0` and `1.0` that tells the client how valuable the context is relative to its budget. The set is fixed at five: there is no `layout` or `sensor` resource. Those live data points are reachable through the `get_layout` and `get_sensor_data` tools instead.

| URI                    | Name             | Priority | Updates                                      |
| ---------------------- | ---------------- | -------- | -------------------------------------------- |
| `hypercolor://state`   | System State     | 0.9      | On every state change                        |
| `hypercolor://effects` | Effect Catalog   | 0.8      | When effects are added or removed            |
| `hypercolor://devices` | Device Inventory | 0.7      | When devices connect or disconnect           |
| `hypercolor://scenes`  | Saved Scenes     | 0.6      | When scenes are created, changed, or deleted |
| `hypercolor://audio`   | Audio Analysis   | 0.4      | ~10 Hz while audio is active                 |

{% <callout type="tip"> %}
Priority orders attention, not freshness. `hypercolor://state` (0.9) is the resource an agent should read first; `hypercolor://audio` (0.4) is the lowest-priority because it is high-frequency telemetry, not durable context. Read state and devices to orient, reach for audio only when building something reactive.
{% </callout> %}

## hypercolor://state

The single most useful resource. A compact snapshot of the daemon: whether it is running or paused, global brightness, live FPS metrics, the active effect, effect and scene counts, device counts, and which input sources are enabled. Its JSON payload is built by the same function as the zero-argument `get_status` tool, so the two are byte-for-byte identical.

{% <api_endpoint method="GET" path="hypercolor://state"> %}
Current daemon state. Read this first to orient before making any change.
{% </api_endpoint> %}

```json
{
  "running": true,
  "paused": false,
  "brightness": 100,
  "fps": {
    "target": 60,
    "capacity": 59.8,
    "delivered": 59.2,
    "actual": 59.8
  },
  "effect": {
    "id": "aurora",
    "name": "Aurora"
  },
  "effect_count": 59,
  "scene_count": 4,
  "devices": {
    "connected": 3,
    "total": 4,
    "total_leds": 212
  },
  "inputs": {
    "audio": "enabled",
    "screen": "disabled",
    "input": "enabled",
    "input_devices_opened": 2,
    "input_devices_denied": 0,
    "input_degraded": null,
    "source_graph_generation": 7,
    "sources": []
  },
  "uptime_seconds": 4127,
  "version": "0.1.0"
}
```

Field notes worth pinning:

- `brightness` is an integer percentage from `0` to `100`, matching the `set_brightness` tool's scale, not a `0.0`-`1.0` float.
- `fps.target` is the current adaptive tier (one of `10`, `20`, `30`, `45`, `60`). `fps.capacity` is a capacity estimate (theoretical throughput derived from smoothed frame time, capped at the tier), and `fps.actual` mirrors it; the measured delivery rate is `fps.delivered`. A gap between target and capacity is the first signal of render pressure. See the [render pipeline](@/architecture/render-pipeline.md) for how the controller shifts tiers.
- `effect` is `null` when nothing is applied, otherwise an object with `id` and `name`.
- `inputs.audio`, `inputs.screen`, and `inputs.input` report whether each input family is enabled. The remaining fields expose device-open counts, permission denials, an optional degradation code, source-graph generation, and per-source status.
- `version` is the daemon's package version, baked in at build time.

## hypercolor://effects

The complete effect catalog: every effect the registry knows about, native and HTML alike, with the metadata an agent needs to choose one.

{% <api_endpoint method="GET" path="hypercolor://effects"> %}
Browse the full effect catalog before applying a visual.
{% </api_endpoint> %}

```json
{
  "effects": [
    {
      "id": "aurora",
      "name": "Aurora",
      "description": "Slow-drifting northern-lights ribbons",
      "category": "ambient",
      "tags": ["calm", "blue", "green", "slow"]
    }
  ],
  "total": 47
}
```

The catalog blends roughly a dozen native Rust built-ins with the SDK's HTML effects, so the `total` is the source of truth for how many are installed on this daemon. Do not hardcode a count in agent logic; read it from the resource. The same data drives the `list_effects` tool, which adds full-text search on top. For building new entries, see [native Rust effects](@/effects/native-rust-effects.md) and [the effects overview](@/effects/_index.md).

{% <callout type="warning"> %}
A `category: "display"` effect is a full-screen HTML face for an LCD, not a per-LED visual. It cannot be applied with `set_effect`; use the `set_display_face` tool instead. The catalog lists faces alongside regular effects, so filter on `category` before applying.
{% </callout> %}

## hypercolor://devices

The MCP hardware inventory: every known device with its connection status, driver origin, presentation, transport, LED count, and segment count. The same payload builder serves this resource and an unfiltered `get_devices` call, so the two JSON values are byte-for-byte identical.

{% <api_endpoint method="GET" path="hypercolor://devices"> %}
Enumerate every known device with connection and hardware detail.
{% </api_endpoint> %}

Abridged entry:

```json
{
  "devices": [
    {
      "id": "razer-huntsman-elite",
      "name": "Razer Huntsman Elite",
      "vendor": "Razer",
      "family": "Razer",
      "transport": "usb",
      "state": "active",
      "led_count": 112,
      "segments": 1
    }
  ],
  "summary": {
    "total": 4,
    "connected": 3,
    "total_leds": 212
  }
}
```

The `summary` block is the quick read: `total` is everything selected by the current view, `connected` counts renderable devices, and `total_leds` sums their LEDs. Each device also carries its full `origin` and `presentation` objects. The `segments` field is a count, not a scene-zone assignment. For connection troubleshooting, see [devices not found](@/troubleshooting/devices-not-found.md).

## hypercolor://scenes

Every reusable scene, including its mutation mode, optional activation side effects, and whether it is currently active.

{% <api_endpoint method="GET" path="hypercolor://scenes"> %}
List saved scenes available to activate.
{% </api_endpoint> %}

```json
{
  "scenes": [
    {
      "id": "evening-calm",
      "name": "Evening calm",
      "description": "Dim blue ambient for the desk",
      "enabled": true,
      "mutation_mode": "snapshot",
      "layout_id": "desk-main",
      "activation_brightness": 0.35,
      "active": false
    }
  ],
  "total": 1
}
```

Each scene reports its `id`, `name`, `description`, `enabled` state, `mutation_mode`, optional `layout_id`, optional `activation_brightness`, and `active` flag. Call `activate_scene` with its name or id to make it live.

{% <callout type="info"> %}
Hypercolor stores reusable lighting state as scenes. Snapshot-mode scenes preserve a captured configuration and block runtime actions from rewriting it. Hypercolor does not schedule scenes itself, so an external automation system calls `activate_scene` when its conditions match.
{% </callout> %}

## hypercolor://audio

Real-time audio analysis straight off the spectrum channel: overall level, the bass/mid/treble split, beat detection, and a compact spectrum summary. This is the resource for building audio-reactive behavior.

{% <api_endpoint method="GET" path="hypercolor://audio"> %}
Live audio levels, beat state, and spectrum summary at roughly 10 Hz.
{% </api_endpoint> %}

```json
{
  "enabled": true,
  "source": "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor",
  "sample_rate": 2048,
  "levels": {
    "overall": 0.61,
    "bass": 0.78,
    "mid": 0.42,
    "treble": 0.19
  },
  "beat": {
    "detected": true,
    "confidence": 0.83,
    "bpm_estimate": 128.0
  },
  "spectrum_summary": {
    "bins": 64
  }
}
```

Field notes:

- `enabled` reflects the daemon's audio config. When audio is off, `source` and `sample_rate` are `null` and the levels read zero.
- `source` is the configured capture device. For reactivity to work this must be a **monitor** source (what the speakers are playing), not a microphone. See [audio setup](@/guide/audio-setup.md) for choosing the right PipeWire or PulseAudio device.
- `sample_rate` is the configured FFT size, not a hardware Hz value.
- `levels` are normalized energies in `[0.0, 1.0]` per band.
- `beat.bpm_estimate` is `null` until the detector locks a tempo.
- `spectrum_summary.bins` reports how many spectrum bins are available; the full per-bin data streams over the WebSocket spectrum channel rather than this resource.

{% <callout type="warning"> %}
This resource updates at roughly 10 Hz, not at the render frame rate. It is a summary for decision-making, not a per-frame signal. An effect that needs frame-accurate audio reads it inside the render loop through the SDK's [audio API](@/effects/audio.md), not by polling this resource.
{% </callout> %}

## How resources relate to tools

Resources and tools cover overlapping ground on purpose. The split is action versus context.

{% <mermaid> %}
graph LR
A[Agent] -->|reads ambient| R[hypercolor:// resources]
A -->|takes action| T[MCP tools]
R --> S[state]
R --> E[effects]
R --> D[devices]
T -->|mutates| ENG[Engine]
R -.snapshot of.-> ENG
{% </mermaid> %}

Read `hypercolor://state` to orient, browse `hypercolor://effects` to choose, then call a tool such as `set_effect` to act. The state resource will reflect the change on its next read, since every surface operates on the same engine through the event bus. For the action side of the contract, see the [tools reference](@/agents/tools-reference.md); for guided multi-step playbooks, see [agent workflows](@/agents/workflows.md).
