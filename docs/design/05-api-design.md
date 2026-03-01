# 05 — API Design

> Every surface Hypercolor speaks through — REST, WebSocket, MCP, D-Bus, Unix socket, webhooks.

---

## Table of Contents

1. [Design Philosophy](#1-design-philosophy)
2. [REST API](#2-rest-api)
3. [WebSocket API](#3-websocket-api)
4. [MCP Server](#4-mcp-server)
5. [D-Bus Interface](#5-d-bus-interface)
6. [CLI Protocol](#6-cli-protocol)
7. [External Integrations](#7-external-integrations)
8. [Event Model](#8-event-model)
9. [Security Model](#9-security-model)
10. [Persona Scenarios](#10-persona-scenarios)
11. [API Versioning & Deprecation](#11-api-versioning--deprecation)

---

## 1. Design Philosophy

Hypercolor exposes multiple API surfaces because different consumers have fundamentally different needs. A Web UI needs 60fps binary frame data. A CLI needs terse JSON over a Unix socket. Home Assistant needs REST webhooks. Claude needs natural language tool mappings. Each surface is purpose-built, but they all share the same event bus, state model, and resource semantics.

**Core principles:**

- **Resource-oriented** — Devices, effects, profiles, layouts, scenes, and inputs are first-class resources with stable IDs, CRUD semantics, and consistent representations across all surfaces.
- **Event-driven** — State changes propagate through a unified event bus. Every API surface subscribes to the same stream. No polling required.
- **Local-first** — The daemon runs on the same machine as the hardware. Network access is opt-in, not assumed. The default path is zero-auth Unix socket or localhost HTTP.
- **Binary where it matters** — Frame data and audio spectra use compact binary formats. Everything else is JSON.
- **Progressive complexity** — Simple things are simple (`POST /effects/aurora/apply`). Power features are discoverable, not required.

**Naming conventions:**

- REST: `snake_case` for JSON properties, kebab-case for URL slugs
- WebSocket: `camelCase` for message types (matches JS convention)
- D-Bus: `PascalCase` methods, `camelCase` properties (freedesktop convention)
- MCP: `snake_case` tool names (MCP SDK convention)

**Base configuration:**

| Surface | Default Binding | Protocol |
|---------|----------------|----------|
| REST API | `127.0.0.1:9420` | HTTP/1.1 + HTTP/2 |
| WebSocket | `ws://127.0.0.1:9420/ws` | WebSocket (RFC 6455) |
| Web UI | `http://127.0.0.1:9420/` | Embedded SvelteKit |
| Unix socket | `/run/hypercolor/hypercolor.sock` | Custom JSON-RPC |
| D-Bus | `tech.hyperbliss.Hypercolor1` | D-Bus session bus |
| MCP | stdio (default) or SSE transport | MCP SDK |

---

## 2. REST API

### 2.1 General Conventions

**Base URL:** `http://127.0.0.1:9420/api/v1`

**Content type:** `application/json` for all request/response bodies.

**Response envelope:**

```json
{
  "data": { ... },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_a1b2c3d4",
    "timestamp": "2026-03-01T12:00:00Z"
  }
}
```

**Error envelope:**

```json
{
  "error": {
    "code": "not_found",
    "message": "Effect 'nonexistent' does not exist",
    "details": {}
  },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_a1b2c3d4",
    "timestamp": "2026-03-01T12:00:00Z"
  }
}
```

**Standard error codes:**

| HTTP Status | Code | Meaning |
|------------|------|---------|
| 400 | `bad_request` | Malformed request body or invalid parameters |
| 401 | `unauthorized` | Missing or invalid API key (network access only) |
| 403 | `forbidden` | Insufficient permissions for this operation |
| 404 | `not_found` | Resource does not exist |
| 409 | `conflict` | State conflict (e.g., device already connected) |
| 422 | `validation_error` | Request body fails schema validation |
| 429 | `rate_limited` | Too many requests (network access only) |
| 500 | `internal_error` | Unexpected daemon error |
| 503 | `unavailable` | Daemon is starting up or shutting down |

**Pagination** (for list endpoints):

```
GET /api/v1/effects?offset=0&limit=50&sort=name&order=asc
```

Response includes pagination metadata:

```json
{
  "data": {
    "items": [...],
    "pagination": {
      "offset": 0,
      "limit": 50,
      "total": 234,
      "has_more": true
    }
  }
}
```

**Filtering** (query parameter syntax):

```
GET /api/v1/effects?audio_reactive=true&author=hyperb1iss
GET /api/v1/devices?status=connected&backend=wled
```

**Search:**

```
GET /api/v1/effects?q=aurora
```

Full-text search across name, description, author, and tags.

### 2.2 Resource Model

#### Devices

A device represents a physical RGB controller or light. Each device has one or more zones (channels, strips, rings).

```
GET    /api/v1/devices                    # List all devices
GET    /api/v1/devices/:id                # Get device details
PATCH  /api/v1/devices/:id                # Update device config (name, enabled)
DELETE /api/v1/devices/:id                # Remove device (stops tracking)
POST   /api/v1/devices/discover           # Trigger device discovery scan
GET    /api/v1/devices/:id/zones          # List zones on a device
GET    /api/v1/devices/:id/zones/:zone_id # Get zone details
PATCH  /api/v1/devices/:id/zones/:zone_id # Update zone config (LED count, color order)
```

**Device object:**

```json
{
  "id": "wled_living_room_strip",
  "name": "Living Room Strip",
  "backend": "wled",
  "status": "connected",
  "firmware_version": "0.15.3",
  "total_leds": 120,
  "zones": [
    {
      "id": "zone_0",
      "name": "Main Strip",
      "led_count": 120,
      "topology": "strip",
      "color_order": "grb"
    }
  ],
  "connection": {
    "type": "ddp",
    "address": "192.168.1.42",
    "port": 4048
  },
  "last_seen": "2026-03-01T12:00:00Z",
  "metadata": {
    "manufacturer": "WLED",
    "model": "ESP32",
    "mac_address": "AA:BB:CC:DD:EE:FF"
  }
}
```

**Discovery response:**

```json
{
  "data": {
    "scan_id": "scan_f8e7d6c5",
    "status": "scanning",
    "backends": ["wled", "openrgb", "hid", "hue"],
    "found": []
  }
}
```

Discovery is async — the client polls or subscribes via WebSocket for `DeviceDiscovered` events.

#### Effects

An effect is a visual program (HTML/Canvas, WGSL shader, or native Rust) that renders to the 320x200 canvas.

```
GET    /api/v1/effects                      # List all effects
GET    /api/v1/effects/:id                  # Get effect details + controls schema
POST   /api/v1/effects/:id/apply            # Apply effect (start rendering)
GET    /api/v1/effects/current              # Get currently active effect
PATCH  /api/v1/effects/current/controls     # Update control values on the active effect
GET    /api/v1/effects/:id/presets          # List presets for an effect
POST   /api/v1/effects/:id/presets          # Save current control values as a preset
PATCH  /api/v1/effects/:id/presets/:name    # Update a preset
DELETE /api/v1/effects/:id/presets/:name    # Delete a preset
POST   /api/v1/effects/:id/presets/:name/apply  # Apply a preset
POST   /api/v1/effects/next                 # Next in history
POST   /api/v1/effects/previous             # Previous in history
POST   /api/v1/effects/shuffle              # Random effect
```

**Effect object:**

```json
{
  "id": "aurora",
  "name": "Aurora",
  "description": "The colors of the Northern Lights illuminate your devices. v2.0",
  "author": "SignalRGB",
  "engine": "servo",
  "category": "ambient",
  "tags": ["nature", "calm", "gradient"],
  "audio_reactive": false,
  "source": "community",
  "thumbnail_url": "/api/v1/effects/aurora/thumbnail",
  "controls": [
    {
      "id": "effectSpeed",
      "label": "Animation Speed",
      "type": "number",
      "min": 0,
      "max": 100,
      "default": 40,
      "value": 40
    },
    {
      "id": "amount",
      "label": "Aurora Density",
      "type": "number",
      "min": 0,
      "max": 100,
      "default": 61,
      "value": 61
    },
    {
      "id": "frontColor",
      "label": "Main Aurora Color",
      "type": "color",
      "default": "#00ffff",
      "value": "#00ffff"
    },
    {
      "id": "backColor",
      "label": "Background Color",
      "type": "color",
      "default": "#005f49",
      "value": "#005f49"
    },
    {
      "id": "colorCycle",
      "label": "Color Cycle",
      "type": "boolean",
      "default": false,
      "value": false
    },
    {
      "id": "cycleSpeed",
      "label": "Color Cycle Speed",
      "type": "number",
      "min": 0,
      "max": 100,
      "default": 50,
      "value": 50
    }
  ],
  "presets": [
    { "name": "Northern Lights", "is_default": true },
    { "name": "Deep Ocean", "is_default": false }
  ]
}
```

**Apply effect with control overrides:**

```http
POST /api/v1/effects/aurora/apply
Content-Type: application/json

{
  "controls": {
    "effectSpeed": 70,
    "frontColor": "#ff00ff",
    "colorCycle": true
  },
  "transition": {
    "type": "crossfade",
    "duration_ms": 500
  }
}
```

**Update controls on the active effect:**

```http
PATCH /api/v1/effects/current/controls
Content-Type: application/json

{
  "effectSpeed": 85,
  "amount": 30
}
```

#### Profiles

A profile captures a complete system state: which effect is running, its control values, which devices are active, the spatial layout, and input source configuration.

```
GET    /api/v1/profiles                     # List all profiles
GET    /api/v1/profiles/:id                 # Get profile details
POST   /api/v1/profiles                     # Create profile from current state
PUT    /api/v1/profiles/:id                 # Update profile
DELETE /api/v1/profiles/:id                 # Delete profile
POST   /api/v1/profiles/:id/apply           # Apply profile
POST   /api/v1/profiles/snapshot            # Save current state as a new profile
```

**Profile object:**

```json
{
  "id": "gaming",
  "name": "Gaming Mode",
  "description": "High-energy reactive lighting for competitive gaming",
  "created_at": "2026-02-15T20:30:00Z",
  "updated_at": "2026-02-28T14:00:00Z",
  "effect": {
    "id": "audio-pulse",
    "controls": {
      "visualStyle": "Vortex",
      "colorScheme": "Cyberpunk",
      "sensitivity": 80,
      "bassBoost": 120
    }
  },
  "layout_id": "main_setup",
  "devices": {
    "wled_living_room_strip": { "enabled": true },
    "prism8_case": { "enabled": true },
    "hue_desk_lamp": { "enabled": false }
  },
  "inputs": {
    "audio": { "enabled": true, "source": "default" },
    "screen": { "enabled": false }
  },
  "brightness": 100
}
```

#### Layouts

A spatial layout defines how device zones map onto the 320x200 effect canvas.

```
GET    /api/v1/layouts                      # List all layouts
GET    /api/v1/layouts/:id                  # Get layout details (full zone positions)
POST   /api/v1/layouts                      # Create layout
PUT    /api/v1/layouts/:id                  # Update layout
DELETE /api/v1/layouts/:id                  # Delete layout
POST   /api/v1/layouts/:id/apply            # Set as active layout
GET    /api/v1/layouts/current              # Get currently active layout
```

**Layout object:**

```json
{
  "id": "main_setup",
  "name": "Main Desk Setup",
  "canvas_width": 320,
  "canvas_height": 200,
  "zones": [
    {
      "device_id": "wled_living_room_strip",
      "zone_id": "zone_0",
      "position": { "x": 0.0, "y": 0.9 },
      "size": { "w": 1.0, "h": 0.1 },
      "rotation": 0.0,
      "topology": "strip",
      "led_count": 120,
      "mirror": false,
      "reverse": false
    },
    {
      "device_id": "prism8_case",
      "zone_id": "channel_0",
      "position": { "x": 0.7, "y": 0.3 },
      "size": { "w": 0.2, "h": 0.4 },
      "rotation": 90.0,
      "topology": "strip",
      "led_count": 60,
      "mirror": false,
      "reverse": true
    }
  ]
}
```

#### Scenes

A scene is a scheduled or triggered profile application. Scenes enable time-based and event-based automation.

```
GET    /api/v1/scenes                       # List all scenes
GET    /api/v1/scenes/:id                   # Get scene details
POST   /api/v1/scenes                       # Create scene
PUT    /api/v1/scenes/:id                   # Update scene
DELETE /api/v1/scenes/:id                   # Delete scene
POST   /api/v1/scenes/:id/activate          # Manually trigger a scene
PATCH  /api/v1/scenes/:id/enabled           # Enable/disable scene
```

**Scene object:**

```json
{
  "id": "sunset_warm",
  "name": "Sunset Warmth",
  "enabled": true,
  "profile_id": "warm_ambient",
  "trigger": {
    "type": "schedule",
    "schedule": {
      "type": "solar",
      "event": "sunset",
      "offset_minutes": -15
    }
  },
  "conditions": [
    {
      "type": "time_range",
      "after": "16:00",
      "before": "23:00"
    }
  ],
  "transition": {
    "type": "crossfade",
    "duration_ms": 3000
  }
}
```

**Trigger types:**

| Type | Fields | Description |
|------|--------|-------------|
| `schedule` | `cron` or `solar` | Time-based (cron expression or solar events) |
| `webhook` | `secret` | External HTTP trigger |
| `event` | `event_type`, `filter` | React to internal events |
| `device` | `device_id`, `state` | Device connect/disconnect |
| `input` | `source`, `threshold` | Audio level, beat detection |

#### Input Sources

```
GET    /api/v1/inputs                       # List available input sources
GET    /api/v1/inputs/:id                   # Get input source details + status
PATCH  /api/v1/inputs/:id                   # Configure input source
POST   /api/v1/inputs/:id/enable            # Enable input source
POST   /api/v1/inputs/:id/disable           # Disable input source
GET    /api/v1/inputs/audio/spectrum         # Get current audio spectrum snapshot
GET    /api/v1/inputs/audio/config           # Get audio analysis config
PATCH  /api/v1/inputs/audio/config           # Update audio analysis config
```

**Audio input object:**

```json
{
  "id": "audio_default",
  "type": "audio",
  "name": "System Audio",
  "enabled": true,
  "status": "active",
  "device_name": "PipeWire Multimedia (default)",
  "sample_rate": 48000,
  "config": {
    "fft_size": 2048,
    "smoothing": 0.7,
    "noise_gate": 0.02,
    "frequency_range": { "min": 20, "max": 20000 },
    "gain": 1.0,
    "beat_sensitivity": 0.6
  },
  "current_levels": {
    "level": 0.42,
    "bass": 0.71,
    "mid": 0.35,
    "treble": 0.18,
    "beat": false,
    "beat_confidence": 0.45
  }
}
```

#### System State

```
GET    /api/v1/state                        # Full daemon state snapshot
GET    /api/v1/state/health                 # Health check (for load balancers, HA)
GET    /api/v1/state/metrics                # Prometheus-style metrics
PATCH  /api/v1/state/brightness             # Set global brightness (0-100)
PATCH  /api/v1/state/fps                    # Set target frame rate
POST   /api/v1/state/pause                  # Pause rendering (all LEDs off)
POST   /api/v1/state/resume                 # Resume rendering
```

**State snapshot:**

```json
{
  "data": {
    "running": true,
    "paused": false,
    "brightness": 85,
    "fps": {
      "target": 60,
      "actual": 59.7
    },
    "effect": {
      "id": "aurora",
      "name": "Aurora"
    },
    "profile": {
      "id": "chill",
      "name": "Chill Mode"
    },
    "layout": {
      "id": "main_setup",
      "name": "Main Desk Setup"
    },
    "devices": {
      "connected": 5,
      "total_leds": 842
    },
    "inputs": {
      "audio": "active",
      "screen": "disabled"
    },
    "uptime_seconds": 86423
  }
}
```

**Health check (200 = healthy, 503 = unhealthy):**

```json
{
  "status": "healthy",
  "version": "0.1.0",
  "uptime_seconds": 86423,
  "checks": {
    "render_loop": "ok",
    "device_backends": "ok",
    "event_bus": "ok"
  }
}
```

### 2.3 Bulk Operations

For controlling multiple devices or applying changes atomically.

```
POST /api/v1/bulk
Content-Type: application/json

{
  "operations": [
    {
      "method": "PATCH",
      "path": "/devices/wled_strip_1",
      "body": { "enabled": true }
    },
    {
      "method": "PATCH",
      "path": "/devices/wled_strip_2",
      "body": { "enabled": true }
    },
    {
      "method": "POST",
      "path": "/effects/aurora/apply",
      "body": { "controls": { "effectSpeed": 60 } }
    }
  ],
  "atomic": true
}
```

Response:

```json
{
  "data": {
    "results": [
      { "index": 0, "status": 200, "data": { ... } },
      { "index": 1, "status": 200, "data": { ... } },
      { "index": 2, "status": 200, "data": { ... } }
    ],
    "all_succeeded": true
  }
}
```

### 2.4 OpenAPI Specification

The daemon serves an OpenAPI 3.1 spec at `/api/v1/openapi.json` and an interactive Swagger UI at `/api/v1/docs`. Generated from Rust types using the `utoipa` crate with `utoipa-swagger-ui` for the Axum integration.

```rust
// Example: derive-based OpenAPI generation
#[derive(ToSchema, Serialize)]
pub struct DeviceResponse {
    pub id: String,
    pub name: String,
    pub backend: Backend,
    pub status: DeviceStatus,
    pub total_leds: u32,
    pub zones: Vec<ZoneInfo>,
}

#[utoipa::path(
    get,
    path = "/api/v1/devices/{id}",
    params(("id" = String, Path, description = "Device identifier")),
    responses(
        (status = 200, description = "Device details", body = DeviceResponse),
        (status = 404, description = "Device not found", body = ErrorResponse),
    ),
    tag = "devices"
)]
async fn get_device(Path(id): Path<String>, State(state): State<AppState>) -> impl IntoResponse {
    // ...
}
```

### 2.5 Rate Limiting

Rate limiting applies only to network-accessible API (when `bind_address` is not `127.0.0.1`).

| Tier | Limit | Scope |
|------|-------|-------|
| Read operations | 120 req/min | Per IP |
| Write operations | 60 req/min | Per IP |
| Frame data (WebSocket) | Unlimited | N/A |
| Bulk operations | 10 req/min | Per IP |
| Discovery scans | 2 req/min | Global |

Implemented via `tower::limit::RateLimitLayer` or `governor` crate. Localhost (`127.0.0.1`, `::1`, Unix socket) is always unlimited.

Rate limit headers:

```
X-RateLimit-Limit: 120
X-RateLimit-Remaining: 117
X-RateLimit-Reset: 1709294460
```

---

## 3. WebSocket API

The WebSocket endpoint at `ws://127.0.0.1:9420/ws` is the primary real-time channel. It carries both high-frequency binary data (LED frames, audio spectra) and JSON event messages.

### 3.1 Connection & Handshake

```
GET /ws HTTP/1.1
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Protocol: hypercolor-v1
```

On connection, the server sends a `hello` message with the current state:

```json
{
  "type": "hello",
  "version": "1.0",
  "state": {
    "running": true,
    "paused": false,
    "brightness": 85,
    "fps": { "target": 60, "actual": 59.7 },
    "effect": { "id": "aurora", "name": "Aurora" },
    "profile": { "id": "chill", "name": "Chill Mode" },
    "device_count": 5,
    "total_leds": 842
  },
  "capabilities": ["frames", "spectrum", "events", "commands"]
}
```

### 3.2 Subscription Model

Clients subscribe to specific channels to control bandwidth. By default, only `events` is subscribed.

```json
{
  "type": "subscribe",
  "channels": ["frames", "spectrum", "events"],
  "config": {
    "frames": {
      "fps": 30,
      "format": "binary",
      "zones": ["all"]
    },
    "spectrum": {
      "fps": 30,
      "bins": 64
    }
  }
}
```

**Available channels:**

| Channel | Data Type | Default FPS | Description |
|---------|-----------|-------------|-------------|
| `frames` | Binary | 30 | LED color data for all zones |
| `spectrum` | Binary | 30 | Audio FFT spectrum data |
| `events` | JSON | N/A (push) | System events (device, effect, profile changes) |
| `canvas` | Binary | 15 | Raw 320x200 canvas pixels (for UI preview) |
| `metrics` | JSON | 1 | Performance metrics (FPS, latency, memory) |

Unsubscribe:

```json
{
  "type": "unsubscribe",
  "channels": ["canvas"]
}
```

### 3.3 Binary Frame Format

LED frame data uses a compact binary format to minimize bandwidth. A binary WebSocket message begins with a 1-byte type discriminator.

**Frame message (type `0x01`):**

```
Byte 0:     0x01 (frame type)
Bytes 1-4:  frame_number (u32 LE)
Bytes 5-8:  timestamp_ms (u32 LE) — millis since daemon start
Byte 9:     zone_count (u8)

For each zone:
  Bytes 0-1:  zone_id_length (u16 LE)
  Bytes 2-N:  zone_id (UTF-8)
  Bytes N+1-N+2: led_count (u16 LE)
  Bytes N+3-...: RGB triplets (led_count * 3 bytes)
```

For a typical setup with 842 LEDs across 5 zones, a frame message is approximately `9 + (5 * ~16) + (842 * 3) = 2,615 bytes` per frame. At 30fps, that's **~78 KB/s** — negligible.

**Spectrum message (type `0x02`):**

```
Byte 0:     0x02 (spectrum type)
Bytes 1-4:  timestamp_ms (u32 LE)
Byte 5:     bin_count (u8) — number of frequency bins
Bytes 6-9:  level (f32 LE) — overall RMS level
Bytes 10-13: bass (f32 LE)
Bytes 14-17: mid (f32 LE)
Bytes 18-21: treble (f32 LE)
Byte 22:    beat (u8, 0 or 1)
Bytes 23-26: beat_confidence (f32 LE)
Bytes 27-...: bins (bin_count * f32 LE)
```

With 64 bins: `27 + 256 = 283 bytes` per message. At 30fps: **~8.5 KB/s**.

**Canvas message (type `0x03`):**

```
Byte 0:     0x03 (canvas type)
Bytes 1-4:  frame_number (u32 LE)
Bytes 5-8:  timestamp_ms (u32 LE)
Bytes 9-10: width (u16 LE) — 320
Bytes 11-12: height (u16 LE) — 200
Byte 13:    format (u8) — 0=RGB, 1=RGBA
Bytes 14-...: pixel data (width * height * 3 or 4)
```

Full canvas at RGB: `14 + 192,000 = 192,014 bytes`. At 15fps: **~2.8 MB/s**. Only subscribe when the spatial editor is open.

### 3.4 JSON Event Messages

Event messages use a consistent envelope:

```json
{
  "type": "event",
  "event": "effect_changed",
  "timestamp": "2026-03-01T12:00:00.123Z",
  "data": {
    "previous": { "id": "rainbow", "name": "Rainbow" },
    "current": { "id": "aurora", "name": "Aurora" }
  }
}
```

See [Section 8: Event Model](#8-event-model) for the complete event taxonomy.

### 3.5 Bidirectional Commands

Clients can send commands over the WebSocket instead of using REST. The command format mirrors the REST API:

```json
{
  "type": "command",
  "id": "cmd_001",
  "method": "POST",
  "path": "/effects/aurora/apply",
  "body": {
    "controls": { "effectSpeed": 70 }
  }
}
```

Response:

```json
{
  "type": "response",
  "id": "cmd_001",
  "status": 200,
  "data": { ... }
}
```

This avoids the overhead of establishing separate HTTP connections for UI interactions that already have an open WebSocket.

### 3.6 Reconnection & State Recovery

When a WebSocket reconnects, the server sends a fresh `hello` message with the current state. There is no message replay — the `hello` provides a complete state snapshot. For frame data, the client simply resumes receiving from the current frame.

**Reconnection strategy (client-side):**

```
Attempt 1: immediate
Attempt 2: 500ms delay
Attempt 3: 1s delay
Attempt 4+: exponential backoff, max 30s
On reconnect: re-send subscribe message, rebuild state from hello
```

### 3.7 Compression

WebSocket `permessage-deflate` is enabled for JSON messages. Binary messages (frames, spectrum, canvas) are sent uncompressed — they're already compact and the compression overhead would add latency.

---

## 4. MCP Server

The Model Context Protocol server lets AI assistants (Claude, GPT, local LLMs) control Hypercolor through natural language. The MCP server runs as a subprocess of the daemon, communicating via stdio by default (or SSE for remote access).

### 4.1 Why MCP for Lighting

Lighting is a perfect MCP domain:

- **Natural language is the right interface** — "Make it a calm blue" is more intuitive than `POST /effects/solid-color/apply { "controls": { "color": "#4488CC" } }`.
- **Context matters** — An AI can interpret "match my music mood" by combining audio analysis, effect selection, and control tuning.
- **Discovery is exploratory** — Users don't know what 230+ effects can do. An AI can browse, suggest, and refine.
- **Composition is creative** — "Create a scene for movie night" requires combining brightness, effect choice, color temperature, and device selection.

### 4.2 MCP Tools

#### Effect Control

**`apply_effect`** — Apply a lighting effect by name or description

```json
{
  "name": "apply_effect",
  "description": "Apply a lighting effect to the RGB setup. Can match by exact name, partial name, or description of the desired visual. Optionally set control parameters.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query": {
        "type": "string",
        "description": "Effect name or natural language description (e.g., 'aurora', 'something with northern lights', 'calm blue waves')"
      },
      "controls": {
        "type": "object",
        "description": "Optional control overrides as key-value pairs (e.g., {\"speed\": 70, \"color\": \"#ff00ff\"})",
        "additionalProperties": true
      },
      "transition_ms": {
        "type": "integer",
        "description": "Crossfade transition duration in milliseconds",
        "default": 500
      }
    },
    "required": ["query"]
  }
}
```

**`set_controls`** — Adjust parameters on the currently running effect

```json
{
  "name": "set_controls",
  "description": "Adjust parameters on the currently active effect. Use list_effect_controls to see available parameters first.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "controls": {
        "type": "object",
        "description": "Control values to update (e.g., {\"speed\": 80, \"colorScheme\": \"Aurora\"})",
        "additionalProperties": true
      }
    },
    "required": ["controls"]
  }
}
```

**`list_effects`** — Browse the effect library

```json
{
  "name": "list_effects",
  "description": "List available lighting effects. Can filter by category, audio reactivity, or search query.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "category": {
        "type": "string",
        "enum": ["ambient", "reactive", "visualizer", "pattern", "nature", "gaming", "holiday"],
        "description": "Filter by effect category"
      },
      "audio_reactive": {
        "type": "boolean",
        "description": "Filter to audio-reactive effects only"
      },
      "query": {
        "type": "string",
        "description": "Search effects by name or description"
      },
      "limit": {
        "type": "integer",
        "description": "Maximum number of results",
        "default": 20
      }
    }
  }
}
```

**`list_effect_controls`** — See what knobs an effect exposes

```json
{
  "name": "list_effect_controls",
  "description": "Get the available control parameters for a specific effect or the currently active effect. Returns parameter names, types, ranges, and current values.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "effect_id": {
        "type": "string",
        "description": "Effect ID. Omit to get controls for the currently active effect."
      }
    }
  }
}
```

#### Device Management

**`list_devices`** — Show connected RGB devices

```json
{
  "name": "list_devices",
  "description": "List all RGB devices with their connection status, LED count, and zone information.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "status": {
        "type": "string",
        "enum": ["all", "connected", "disconnected"],
        "default": "all"
      }
    }
  }
}
```

**`configure_device`** — Enable/disable devices, rename zones

```json
{
  "name": "configure_device",
  "description": "Configure a device: enable/disable it, rename it, or adjust zone settings.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "device_id": {
        "type": "string",
        "description": "Device identifier"
      },
      "enabled": {
        "type": "boolean",
        "description": "Enable or disable the device"
      },
      "name": {
        "type": "string",
        "description": "New display name for the device"
      }
    },
    "required": ["device_id"]
  }
}
```

**`discover_devices`** — Scan for new RGB hardware

```json
{
  "name": "discover_devices",
  "description": "Trigger a scan for new RGB devices across all backends (WLED, OpenRGB, USB HID, Hue). Returns newly discovered devices.",
  "inputSchema": {
    "type": "object",
    "properties": {}
  }
}
```

#### Scene & Profile Management

**`apply_profile`** — Load a saved lighting profile

```json
{
  "name": "apply_profile",
  "description": "Apply a saved lighting profile. Profiles capture the complete lighting state: effect, controls, device selection, brightness, and layout.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query": {
        "type": "string",
        "description": "Profile name or description (e.g., 'gaming', 'movie night', 'chill')"
      }
    },
    "required": ["query"]
  }
}
```

**`create_profile`** — Save the current state as a profile

```json
{
  "name": "create_profile",
  "description": "Save the current lighting state as a named profile for later use.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "name": {
        "type": "string",
        "description": "Profile name"
      },
      "description": {
        "type": "string",
        "description": "What this profile is for"
      }
    },
    "required": ["name"]
  }
}
```

**`create_scene`** — Set up automated lighting triggers

```json
{
  "name": "create_scene",
  "description": "Create an automated lighting scene that triggers based on time, events, or external signals. Links a trigger condition to a profile.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "name": {
        "type": "string",
        "description": "Scene name"
      },
      "profile_id": {
        "type": "string",
        "description": "Profile to activate when triggered"
      },
      "trigger": {
        "type": "object",
        "description": "Trigger configuration",
        "properties": {
          "type": {
            "type": "string",
            "enum": ["schedule", "sunset", "sunrise", "device_connect", "audio_beat"]
          },
          "cron": { "type": "string" },
          "offset_minutes": { "type": "integer" }
        },
        "required": ["type"]
      },
      "transition_ms": {
        "type": "integer",
        "description": "Crossfade duration in milliseconds",
        "default": 1000
      }
    },
    "required": ["name", "profile_id", "trigger"]
  }
}
```

#### System Control

**`set_brightness`** — Adjust global brightness

```json
{
  "name": "set_brightness",
  "description": "Set the global brightness level for all RGB devices.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "brightness": {
        "type": "integer",
        "minimum": 0,
        "maximum": 100,
        "description": "Brightness percentage (0 = off, 100 = full)"
      }
    },
    "required": ["brightness"]
  }
}
```

**`get_state`** — Get current system state

```json
{
  "name": "get_state",
  "description": "Get the current state of the Hypercolor daemon: running effect, brightness, connected devices, active profile, and performance metrics.",
  "inputSchema": {
    "type": "object",
    "properties": {}
  }
}
```

**`pause_resume`** — Pause or resume lighting

```json
{
  "name": "pause_resume",
  "description": "Pause rendering (all LEDs go dark) or resume from pause.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "action": {
        "type": "string",
        "enum": ["pause", "resume", "toggle"]
      }
    },
    "required": ["action"]
  }
}
```

#### Creative Tools

**`suggest_effect`** — AI-powered effect recommendation

```json
{
  "name": "suggest_effect",
  "description": "Get effect suggestions based on a mood, activity, or aesthetic description. Returns ranked suggestions with explanations.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "mood": {
        "type": "string",
        "description": "Desired mood or vibe (e.g., 'relaxing', 'energetic', 'spooky', 'focused')"
      },
      "activity": {
        "type": "string",
        "description": "What the user is doing (e.g., 'gaming', 'coding', 'watching a movie', 'hosting a party')"
      },
      "colors": {
        "type": "array",
        "items": { "type": "string" },
        "description": "Preferred colors (e.g., ['blue', 'purple'] or ['#ff00ff'])"
      },
      "audio_reactive": {
        "type": "boolean",
        "description": "Whether the effect should react to music/audio"
      }
    }
  }
}
```

### 4.3 MCP Resources

Resources provide read-only context that the AI can reference.

**`hypercolor://state`** — Current daemon state (same as `get_state` tool output)

**`hypercolor://devices`** — Full device inventory with zone details

**`hypercolor://effects`** — Complete effect catalog with metadata

**`hypercolor://profiles`** — All saved profiles

**`hypercolor://audio`** — Current audio analysis data (levels, beat, spectrum summary)

**`hypercolor://effect/{id}`** — Detailed metadata for a specific effect, including all controls

Resource URIs are subscribed via the MCP resource subscription protocol, so the AI receives updates when resources change.

### 4.4 MCP Prompts

Prompts provide pre-built interaction patterns.

**`mood_lighting`** — Help the user set up mood-based lighting

```json
{
  "name": "mood_lighting",
  "description": "Help the user describe the lighting mood they want and configure Hypercolor to match",
  "arguments": [
    {
      "name": "mood",
      "description": "Starting mood description (optional — the prompt will ask if not provided)",
      "required": false
    }
  ]
}
```

Prompt template:
```
You are helping configure Hypercolor RGB lighting. The user wants: {{mood}}

Available effects: {{resource:hypercolor://effects}}
Current state: {{resource:hypercolor://state}}
Connected devices: {{resource:hypercolor://devices}}

Suggest an effect and control settings that match the requested mood.
Consider the user's hardware setup and which effects work best with their device count and layout.
```

**`troubleshoot`** — Diagnose device connection issues

```json
{
  "name": "troubleshoot",
  "description": "Diagnose and fix Hypercolor device connection or rendering issues",
  "arguments": [
    {
      "name": "issue",
      "description": "Description of the problem",
      "required": true
    }
  ]
}
```

**`setup_automation`** — Walk through creating a scene

```json
{
  "name": "setup_automation",
  "description": "Help the user create automated lighting scenes with triggers and conditions",
  "arguments": []
}
```

### 4.5 Natural Language Mapping

The MCP server includes a semantic matching layer for effect queries. When `apply_effect` receives a natural language query like "calm blue aurora", the matching pipeline:

1. **Exact match** — Check if query matches an effect name exactly
2. **Fuzzy match** — Levenshtein distance against all effect names
3. **Tag match** — Check if query words appear in effect tags
4. **Semantic match** — Compare query against effect descriptions using keyword extraction

This is intentionally simple — no embedding model required. The AI assistant itself provides the semantic reasoning. The daemon just needs good fuzzy matching.

```rust
pub fn match_effect(query: &str, effects: &[EffectMetadata]) -> Vec<EffectMatch> {
    let mut matches = Vec::new();

    for effect in effects {
        let score = max(
            exact_match_score(query, &effect.name),
            max(
                fuzzy_match_score(query, &effect.name),
                max(
                    tag_match_score(query, &effect.tags),
                    description_match_score(query, &effect.description),
                ),
            ),
        );

        if score > 0.3 {
            matches.push(EffectMatch { effect, score });
        }
    }

    matches.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    matches
}
```

### 4.6 Implementation

Built on the `rmcp` crate (Rust MCP SDK). The MCP server runs as a tokio task within the daemon process, sharing the same `HypercolorBus` and state.

```rust
use rmcp::{ServerHandler, tool, resource, prompt};

#[derive(Clone)]
pub struct HypercolorMcp {
    bus: HypercolorBus,
    state: Arc<RwLock<DaemonState>>,
}

#[tool(name = "apply_effect", description = "Apply a lighting effect")]
async fn apply_effect(&self, query: String, controls: Option<HashMap<String, Value>>) -> Result<String> {
    let effects = self.state.read().await.effects.list();
    let matches = match_effect(&query, &effects);

    if matches.is_empty() {
        return Ok(format!("No effects matching '{}'. Try list_effects to browse.", query));
    }

    let best = &matches[0];
    self.state.write().await.apply_effect(&best.effect.id, controls).await?;

    Ok(format!(
        "Applied '{}' (matched with {:.0}% confidence).{}",
        best.effect.name,
        best.score * 100.0,
        if matches.len() > 1 {
            format!(" Other matches: {}", matches[1..].iter().take(3)
                .map(|m| m.effect.name.as_str()).collect::<Vec<_>>().join(", "))
        } else {
            String::new()
        }
    ))
}
```

### 4.7 Transport Modes

| Mode | Use Case | Configuration |
|------|----------|---------------|
| **stdio** | Claude Code, local AI tools | Default. Daemon spawns MCP as subprocess |
| **SSE** | Remote AI access, web-based agents | `--mcp-transport sse --mcp-port 9421` |
| **Streamable HTTP** | Modern MCP clients | `--mcp-transport http --mcp-port 9421` |

---

## 5. D-Bus Interface

D-Bus provides desktop integration — GNOME extensions, KDE widgets, systemd control, and hotkey-driven lighting changes without needing the full web UI.

### 5.1 Service Definition

**Bus name:** `tech.hyperbliss.Hypercolor1`
**Object path:** `/tech/hyperbliss/Hypercolor1`

Follows freedesktop.org conventions: reverse-domain bus name, version-suffixed to allow breaking changes.

### 5.2 Interfaces

#### `tech.hyperbliss.Hypercolor1.Daemon`

Core daemon control and state queries.

**Methods:**

| Method | Signature | Description |
|--------|-----------|-------------|
| `GetState` | `() -> a{sv}` | Returns daemon state as a variant dict |
| `Pause` | `()` | Pause rendering |
| `Resume` | `()` | Resume rendering |
| `SetBrightness` | `(u)` | Set global brightness (0-100) |
| `GetBrightness` | `() -> u` | Get current brightness |

**Properties (readable, some writable):**

| Property | Type | Access | Description |
|----------|------|--------|-------------|
| `Running` | `b` | R | Whether the daemon is running |
| `Paused` | `b` | R | Whether rendering is paused |
| `Brightness` | `u` | RW | Global brightness (0-100) |
| `Fps` | `d` | R | Current actual FPS |
| `TargetFps` | `u` | RW | Target FPS |
| `Version` | `s` | R | Daemon version string |
| `Uptime` | `t` | R | Uptime in seconds |

#### `tech.hyperbliss.Hypercolor1.Effects`

Effect management.

**Methods:**

| Method | Signature | Description |
|--------|-----------|-------------|
| `ListEffects` | `() -> a(ss)` | Returns array of (id, name) tuples |
| `GetCurrentEffect` | `() -> a{sv}` | Current effect info |
| `ApplyEffect` | `(s)` | Apply effect by ID |
| `ApplyEffectWithControls` | `(sa{sv})` | Apply effect with control overrides |
| `SetControl` | `(sv)` | Set a single control value on active effect |
| `NextEffect` | `()` | Next effect in history |
| `PreviousEffect` | `()` | Previous effect in history |
| `ShuffleEffect` | `()` | Apply random effect |

**Properties:**

| Property | Type | Access | Description |
|----------|------|--------|-------------|
| `CurrentEffectId` | `s` | R | Active effect ID |
| `CurrentEffectName` | `s` | R | Active effect display name |
| `EffectCount` | `u` | R | Total number of available effects |

#### `tech.hyperbliss.Hypercolor1.Devices`

Device management.

**Methods:**

| Method | Signature | Description |
|--------|-----------|-------------|
| `ListDevices` | `() -> a(sssub)` | Returns (id, name, backend, led_count, connected) |
| `GetDevice` | `(s) -> a{sv}` | Full device details by ID |
| `Discover` | `()` | Trigger device discovery scan |
| `EnableDevice` | `(s)` | Enable a device by ID |
| `DisableDevice` | `(s)` | Disable a device by ID |

**Properties:**

| Property | Type | Access | Description |
|----------|------|--------|-------------|
| `DeviceCount` | `u` | R | Number of known devices |
| `ConnectedCount` | `u` | R | Number of currently connected devices |
| `TotalLeds` | `u` | R | Sum of all connected device LEDs |

#### `tech.hyperbliss.Hypercolor1.Profiles`

Profile and scene management.

**Methods:**

| Method | Signature | Description |
|--------|-----------|-------------|
| `ListProfiles` | `() -> a(ss)` | Returns (id, name) tuples |
| `ApplyProfile` | `(s)` | Apply profile by ID |
| `SaveProfile` | `(ss)` | Save current state as profile (name, description) |

### 5.3 Signals

All interfaces emit signals for state changes:

| Signal | Interface | Signature | Description |
|--------|-----------|-----------|-------------|
| `EffectChanged` | Effects | `(ss)` | (effect_id, effect_name) |
| `ControlChanged` | Effects | `(sv)` | (control_id, new_value) |
| `DeviceConnected` | Devices | `(ssu)` | (device_id, device_name, led_count) |
| `DeviceDisconnected` | Devices | `(s)` | (device_id) |
| `ProfileApplied` | Profiles | `(ss)` | (profile_id, profile_name) |
| `BrightnessChanged` | Daemon | `(u)` | (new_brightness) |
| `PausedChanged` | Daemon | `(b)` | (is_paused) |
| `Error` | Daemon | `(ss)` | (error_code, message) |

### 5.4 Implementation

Using `zbus` for ergonomic async D-Bus in Rust:

```rust
use zbus::{interface, fdo, Connection, SignalContext};

struct HypercolorDbus {
    bus: HypercolorBus,
    state: Arc<RwLock<DaemonState>>,
}

#[interface(name = "tech.hyperbliss.Hypercolor1.Effects")]
impl HypercolorDbus {
    async fn apply_effect(&self, effect_id: &str) -> fdo::Result<()> {
        self.state.write().await
            .apply_effect(effect_id, None).await
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    async fn list_effects(&self) -> fdo::Result<Vec<(String, String)>> {
        let effects = self.state.read().await.effects.list();
        Ok(effects.into_iter().map(|e| (e.id, e.name)).collect())
    }

    #[zbus(property)]
    async fn current_effect_name(&self) -> fdo::Result<String> {
        Ok(self.state.read().await.current_effect_name().to_string())
    }

    #[zbus(signal)]
    async fn effect_changed(ctx: &SignalContext<'_>, id: &str, name: &str) -> zbus::Result<()>;
}
```

### 5.5 Desktop Integration Points

**GNOME Extension:**
- Read `CurrentEffectName` and `Brightness` for panel indicator
- Popup with effect list (from `ListEffects`) and brightness slider
- Subscribe to `EffectChanged` signal for live updates
- Keyboard shortcut → `ApplyProfile("gaming")` or `ShuffleEffect()`

**KDE Plasma Widget:**
- Same D-Bus methods, different UI framework
- KDE's system settings → Hypercolor panel with effect browser

**systemd service:**
- `hypercolor.service` — the daemon
- D-Bus activation: daemon starts on first D-Bus method call
- `busctl --user call tech.hyperbliss.Hypercolor1 /tech/hyperbliss/Hypercolor1 tech.hyperbliss.Hypercolor1.Effects ApplyEffect s aurora`

**Hotkey integration:**
```bash
# ~/.config/sway/config
bindsym $mod+F5 exec busctl --user call tech.hyperbliss.Hypercolor1 \
  /tech/hyperbliss/Hypercolor1 tech.hyperbliss.Hypercolor1.Effects ShuffleEffect
bindsym $mod+F6 exec busctl --user call tech.hyperbliss.Hypercolor1 \
  /tech/hyperbliss/Hypercolor1 tech.hyperbliss.Hypercolor1.Daemon SetBrightness u 50
```

---

## 6. CLI Protocol

The CLI (`hypercolor` binary) communicates with the running daemon over a Unix domain socket for maximum speed and zero network overhead. REST is available as a fallback when the daemon is on a remote machine.

### 6.1 Transport Selection

```
Local (default):  /run/hypercolor/hypercolor.sock
Remote (--host):  http://<host>:9420/api/v1
```

The CLI auto-detects: if the socket exists, use it. If `--host` is specified, use REST.

### 6.2 Unix Socket Protocol

The Unix socket uses a simple framed JSON-RPC 2.0 protocol with length-prefixed messages.

**Frame format:**

```
[4 bytes: message length (u32 LE)][JSON-RPC message]
```

**Request:**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "effects.apply",
  "params": {
    "effect_id": "aurora",
    "controls": { "effectSpeed": 70 }
  }
}
```

**Response:**

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

**Error:**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32602,
    "message": "Effect 'nonexistent' not found",
    "data": { "available": ["aurora", "rainbow", "solid-color"] }
  }
}
```

### 6.3 RPC Method Catalog

| Method | Params | Description |
|--------|--------|-------------|
| `state.get` | `{}` | Full daemon state |
| `state.health` | `{}` | Health check |
| `state.set_brightness` | `{ brightness: u8 }` | Set global brightness |
| `state.set_fps` | `{ fps: u32 }` | Set target FPS |
| `state.pause` | `{}` | Pause rendering |
| `state.resume` | `{}` | Resume rendering |
| `effects.list` | `{ query?, category?, audio_reactive?, offset?, limit? }` | List effects |
| `effects.get` | `{ effect_id }` | Effect details |
| `effects.apply` | `{ effect_id, controls?, transition_ms? }` | Apply effect |
| `effects.current` | `{}` | Current effect |
| `effects.set_controls` | `{ controls }` | Update active controls |
| `effects.next` | `{}` | Next in history |
| `effects.previous` | `{}` | Previous in history |
| `effects.shuffle` | `{}` | Random effect |
| `devices.list` | `{ status? }` | List devices |
| `devices.get` | `{ device_id }` | Device details |
| `devices.discover` | `{}` | Trigger discovery |
| `devices.enable` | `{ device_id }` | Enable device |
| `devices.disable` | `{ device_id }` | Disable device |
| `profiles.list` | `{}` | List profiles |
| `profiles.apply` | `{ profile_id }` | Apply profile |
| `profiles.save` | `{ name, description? }` | Save current state |
| `layouts.list` | `{}` | List layouts |
| `layouts.apply` | `{ layout_id }` | Apply layout |
| `inputs.list` | `{}` | List input sources |
| `inputs.audio_config` | `{ config? }` | Get/set audio config |

### 6.4 Streaming (Watch Mode)

For live monitoring, the CLI can subscribe to the event stream:

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "subscribe",
  "params": {
    "channels": ["events"],
    "filter": ["effect_changed", "device_connected", "device_disconnected"]
  }
}
```

The daemon sends JSON-RPC notifications (no `id` field) for each matching event:

```json
{
  "jsonrpc": "2.0",
  "method": "event",
  "params": {
    "type": "effect_changed",
    "timestamp": "2026-03-01T12:00:00Z",
    "data": {
      "previous": { "id": "rainbow", "name": "Rainbow" },
      "current": { "id": "aurora", "name": "Aurora" }
    }
  }
}
```

This powers `hypercolor watch` — a live terminal dashboard of system events.

### 6.5 Shell Completion Data

The daemon provides completion data for dynamic resources (effect names, device IDs, profile names):

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "completions",
  "params": {
    "resource": "effects",
    "prefix": "aur"
  }
}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {
    "completions": [
      { "value": "aurora", "description": "Northern Lights effect" },
      { "value": "aurora-borealis", "description": "Realistic aurora with color cycling" }
    ]
  }
}
```

Shell completion scripts (`hypercolor completion bash/zsh/fish`) generate static completions for subcommands and flags, plus dynamic completions that query the daemon socket.

### 6.6 CLI Command Surface

```
hypercolor daemon [--port 9420] [--no-web] [--bind 0.0.0.0]
hypercolor tui

hypercolor status                              # Quick state summary
hypercolor set <effect> [--control key=val]... # Apply effect
hypercolor get [effect|devices|profile|state]  # Query resources
hypercolor list [effects|devices|profiles|layouts|scenes]
hypercolor search <query>                      # Search effects
hypercolor profile <name>                      # Apply profile
hypercolor profile save <name> [--desc "..."]  # Save profile
hypercolor brightness <0-100>                  # Set brightness
hypercolor pause | resume                      # Toggle rendering
hypercolor discover                            # Scan for devices
hypercolor watch [--filter type1,type2]        # Live event stream
hypercolor completion [bash|zsh|fish]          # Shell completions
```

---

## 7. External Integrations

### 7.1 Home Assistant

Three integration paths, from simplest to most powerful.

#### Path A: REST Entity (Recommended for v1)

A custom Home Assistant integration (`hypercolor-homeassistant`) that wraps the REST API. Ships as a HACS component.

**Entities exposed:**

| Entity Type | Entity ID | Attributes |
|------------|-----------|------------|
| `light` | `light.hypercolor` | `brightness`, `effect`, `effect_list` |
| `select` | `select.hypercolor_profile` | Profile list from `/api/v1/profiles` |
| `select` | `select.hypercolor_layout` | Layout list from `/api/v1/layouts` |
| `button` | `button.hypercolor_next` | Next effect |
| `button` | `button.hypercolor_previous` | Previous effect |
| `button` | `button.hypercolor_shuffle` | Random effect |
| `sensor` | `sensor.hypercolor_fps` | Current FPS |
| `sensor` | `sensor.hypercolor_device_count` | Connected device count |
| `binary_sensor` | `binary_sensor.hypercolor_audio` | Audio input active |

The `light` entity maps naturally to HA's light platform:
- `turn_on` / `turn_off` → `POST /state/resume` / `POST /state/pause`
- `set_brightness` → `PATCH /state/brightness`
- `set_effect` → `POST /effects/{id}/apply`
- `effect_list` → `GET /effects` (names only)

**HA automation example:**

```yaml
automation:
  - alias: "Sunset Warm Lighting"
    trigger:
      platform: sun
      event: sunset
      offset: "-00:15:00"
    action:
      - service: select.select_option
        target:
          entity_id: select.hypercolor_profile
        data:
          option: "Warm Ambient"
```

#### Path B: MQTT (For Users with Existing MQTT Infrastructure)

Optional MQTT publisher built into the daemon. Publishes state to configurable topics and subscribes for commands.

**Topics:**

```
hypercolor/state              → Full state JSON (retained)
hypercolor/effect/current     → Current effect name (retained)
hypercolor/brightness         → Current brightness (retained)
hypercolor/devices/count      → Connected device count (retained)

hypercolor/command/effect     ← Apply effect by name
hypercolor/command/profile    ← Apply profile by name
hypercolor/command/brightness ← Set brightness (0-100)
hypercolor/command/pause      ← Pause rendering
hypercolor/command/resume     ← Resume rendering
```

MQTT discovery messages for Home Assistant auto-detection:

```json
{
  "name": "Hypercolor",
  "unique_id": "hypercolor_main",
  "command_topic": "hypercolor/command/effect",
  "state_topic": "hypercolor/state",
  "brightness_command_topic": "hypercolor/command/brightness",
  "brightness_state_topic": "hypercolor/brightness",
  "effect_list": ["Aurora", "Rainbow", "Solid Color", "..."],
  "effect_command_topic": "hypercolor/command/effect",
  "effect_state_topic": "hypercolor/effect/current"
}
```

#### Path C: Native MCP (Future — HA Already Supports MCP)

Home Assistant has native MCP client support (since 2025.2). The Hypercolor MCP server could register directly with HA's AI agent, enabling natural language lighting control through HA's voice assistant pipeline.

### 7.2 OBS Integration

OBS scene changes trigger lighting profile switches via webhooks.

**OBS WebSocket (obs-websocket v5) → Hypercolor webhook:**

An OBS script (Lua or Python) listens for scene changes and POSTs to the Hypercolor webhook endpoint:

```
POST /api/v1/webhooks/obs
Content-Type: application/json

{
  "event": "scene_changed",
  "scene_name": "Gaming",
  "source": "obs"
}
```

**Webhook configuration (daemon side):**

```toml
# ~/.config/hypercolor/webhooks.toml
[[webhook]]
name = "OBS Scene Change"
source = "obs"
event = "scene_changed"
mappings = [
  { match = "Gaming",      profile = "gaming_mode" },
  { match = "Just Chatting", profile = "chill" },
  { match = "BRB",         profile = "idle_rainbow" },
  { match = "Starting Soon", profile = "stream_intro" },
]
```

**OBS Lua script (runs inside OBS):**

```lua
obs = obslua
local http = require("socket.http")
local json = require("json")

function on_scene_change(event)
    if event == obs.OBS_FRONTEND_EVENT_SCENE_CHANGED then
        local scene = obs.obs_frontend_get_current_scene()
        local name = obs.obs_source_get_name(scene)
        http.request({
            url = "http://127.0.0.1:9420/api/v1/webhooks/obs",
            method = "POST",
            headers = { ["Content-Type"] = "application/json" },
            source = ltn12.source.string(json.encode({
                event = "scene_changed",
                scene_name = name,
                source = "obs"
            }))
        })
        obs.obs_source_release(scene)
    end
end

function script_load(settings)
    obs.obs_frontend_add_event_callback(on_scene_change)
end
```

### 7.3 Stream Deck Integration

Elgato Stream Deck buttons map to Hypercolor actions via the Stream Deck SDK or the community `streamdeck-plugin-template`.

**Stream Deck plugin actions:**

| Action ID | Title | Description |
|-----------|-------|-------------|
| `tech.hyperbliss.hypercolor.set-effect` | Set Effect | Apply a specific effect (configurable) |
| `tech.hyperbliss.hypercolor.set-profile` | Set Profile | Apply a lighting profile |
| `tech.hyperbliss.hypercolor.toggle-pause` | Toggle Pause | Pause/resume rendering |
| `tech.hyperbliss.hypercolor.brightness` | Brightness | Dial to adjust brightness |
| `tech.hyperbliss.hypercolor.next-effect` | Next Effect | Cycle to next effect |
| `tech.hyperbliss.hypercolor.shuffle` | Shuffle | Random effect |

Each action calls the REST API. The plugin uses the WebSocket connection for state feedback — when the effect changes (from any source), the Stream Deck button updates its icon/title.

### 7.4 Generic Webhooks

For IFTTT, Zapier, Node-RED, custom scripts, and anything that can send HTTP.

```
POST /api/v1/webhooks/trigger
Content-Type: application/json

{
  "webhook_id": "whk_abc123",
  "secret": "shared_secret_token",
  "payload": {
    "action": "apply_profile",
    "profile": "movie_night"
  }
}
```

**Webhook registration:**

```
POST /api/v1/webhooks
Content-Type: application/json

{
  "name": "IFTTT Movie Time",
  "action": "apply_profile",
  "profile_id": "movie_night",
  "secret": "auto_generated_if_omitted"
}
```

Response:

```json
{
  "data": {
    "webhook_id": "whk_abc123",
    "secret": "hc_whk_k7m2p9x4...",
    "url": "http://192.168.1.100:9420/api/v1/webhooks/trigger",
    "curl_example": "curl -X POST http://192.168.1.100:9420/api/v1/webhooks/trigger -H 'Content-Type: application/json' -d '{\"webhook_id\":\"whk_abc123\",\"secret\":\"hc_whk_k7m2p9x4...\"}'"
  }
}
```

### 7.5 Scriptable Integration Pattern

For developers who want to build custom integrations, the Python client library provides the cleanest path:

```python
# pip install hypercolor
from hypercolor import HypercolorClient

async def ci_build_lights():
    """Pulse red when CI build fails, green when it passes."""
    async with HypercolorClient() as hc:
        build_status = await check_ci_build()

        if build_status == "failed":
            await hc.apply_effect("solid-color", controls={"color": "#ff0000", "breathe": True})
        elif build_status == "passed":
            await hc.apply_effect("solid-color", controls={"color": "#00ff00", "breathe": False})
```

Client libraries planned for: Python, TypeScript/Node, Rust (via `hypercolor-core` directly).

---

## 8. Event Model

### 8.1 Event Taxonomy

Every state change in Hypercolor produces an event on the internal `tokio::broadcast` bus. All API surfaces (WebSocket, D-Bus signals, Unix socket subscriptions, MQTT) deliver the same events.

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum HypercolorEvent {
    // === Effect Events ===
    EffectChanged {
        previous: Option<EffectRef>,
        current: EffectRef,
        trigger: ChangeTrigger,    // "user", "profile", "scene", "api", "cli"
    },
    EffectControlChanged {
        effect_id: String,
        control_id: String,
        old_value: ControlValue,
        new_value: ControlValue,
    },

    // === Device Events ===
    DeviceConnected {
        device_id: String,
        name: String,
        backend: String,
        led_count: u32,
    },
    DeviceDisconnected {
        device_id: String,
        reason: DisconnectReason,  // "removed", "error", "timeout", "shutdown"
    },
    DeviceDiscoveryStarted {
        backends: Vec<String>,
    },
    DeviceDiscoveryCompleted {
        found: Vec<DeviceRef>,
        duration_ms: u64,
    },
    DeviceError {
        device_id: String,
        error: String,
        recoverable: bool,
    },

    // === Profile Events ===
    ProfileApplied {
        profile_id: String,
        profile_name: String,
        trigger: ChangeTrigger,
    },
    ProfileSaved {
        profile_id: String,
        profile_name: String,
    },
    ProfileDeleted {
        profile_id: String,
    },

    // === Scene Events ===
    SceneTriggered {
        scene_id: String,
        scene_name: String,
        trigger_type: String,
    },
    SceneEnabled {
        scene_id: String,
        enabled: bool,
    },

    // === Layout Events ===
    LayoutChanged {
        previous: Option<String>,
        current: String,
    },
    LayoutUpdated {
        layout_id: String,
    },

    // === Input Events ===
    InputSourceChanged {
        input_id: String,
        input_type: String,
        enabled: bool,
    },
    AudioBeat {
        confidence: f32,
        bpm: Option<f32>,
    },

    // === System Events ===
    BrightnessChanged {
        old: u8,
        new: u8,
    },
    FpsChanged {
        target: u32,
    },
    Paused,
    Resumed,
    DaemonStarted {
        version: String,
        device_count: u32,
    },
    DaemonShutdown {
        reason: String,
    },
    Error {
        code: String,
        message: String,
        severity: Severity,  // "warning", "error", "critical"
    },
    WebhookReceived {
        webhook_id: String,
        source: String,
    },
}
```

### 8.2 Event Priority & Ordering

Events are delivered in causal order within a single category. Cross-category ordering is best-effort.

| Priority | Events | Delivery |
|----------|--------|----------|
| **Critical** | `DaemonShutdown`, `Error(critical)` | Guaranteed delivery, sent before shutdown |
| **High** | `DeviceConnected`, `DeviceDisconnected`, `DeviceError` | Delivered within 1ms |
| **Normal** | `EffectChanged`, `ProfileApplied`, `BrightnessChanged` | Delivered within 5ms |
| **Low** | `AudioBeat`, `DeviceDiscoveryCompleted`, `LayoutUpdated` | Best-effort, may be coalesced |

The `tokio::broadcast` channel has a buffer of 256 events. If a slow subscriber falls behind, it receives a `Lagged(n)` error and can request a state snapshot to recover.

### 8.3 Event Filtering

Each subscriber can filter events. This is especially important for CLI watch mode and WebSocket subscriptions where bandwidth matters.

**Filter by event category:**

```json
{
  "filter": {
    "categories": ["effect", "device"],
    "exclude_types": ["EffectControlChanged"]
  }
}
```

**Filter by device:**

```json
{
  "filter": {
    "device_ids": ["wled_strip_1", "prism8_case"]
  }
}
```

### 8.4 Event Replay for Late Joiners

The event bus does not maintain a replay log — events are fire-and-forget. Late joiners receive:

1. A complete **state snapshot** (equivalent to `GET /state`) on connection
2. All events from the moment of subscription forward

This is intentional. LED lighting is a real-time system where the current state matters more than history. The state snapshot provides everything a new subscriber needs.

For audit/debug purposes, the daemon can optionally log events to a ring buffer file (`--event-log /tmp/hypercolor-events.jsonl`, last 10,000 events).

---

## 9. Security Model

### 9.1 Threat Model

Hypercolor controls lights. It cannot brick hardware, exfiltrate data, or compromise system security. The primary concerns are:

| Threat | Severity | Mitigation |
|--------|----------|------------|
| Unauthorized effect changes | Low | Annoying, not dangerous |
| Excessive API calls (DoS) | Medium | Rate limiting |
| Reading device info | Low | No sensitive data exposed |
| Firmware manipulation | High | Not exposed via API at all |
| Daemon crash via malformed input | Medium | Input validation, fuzzing |
| Network-exposed daemon hijacked | Medium | API keys for non-local access |

### 9.2 Local Access (No Auth)

When bound to `127.0.0.1` (the default), no authentication is required. The reasoning:

- The daemon runs as the user's own process
- Only processes on the same machine can connect
- The Unix socket has filesystem permissions (`0660`, user + group)
- D-Bus session bus is already authenticated per the D-Bus spec

This matches the security model of OpenRGB (TCP 6742, no auth), WLED (HTTP, no auth on local network), and SignalRGB (HTTP API, local only by default).

### 9.3 Network Access (API Key)

When the daemon binds to `0.0.0.0` or a non-loopback address, API key authentication is required.

**Configuration:**

```toml
# ~/.config/hypercolor/daemon.toml
[api]
bind_address = "0.0.0.0"
port = 9420

[api.auth]
enabled = true
# Auto-generated on first run, stored in this file
api_key = "hc_ak_x7k2m9p4q1w8..."
```

**Request header:**

```
Authorization: Bearer hc_ak_x7k2m9p4q1w8...
```

Or query parameter for WebSocket connections:

```
ws://192.168.1.100:9420/ws?token=hc_ak_x7k2m9p4q1w8...
```

### 9.4 Access Tiers

Two tiers, kept simple because this is a lighting daemon, not a bank.

| Tier | Capabilities | Who |
|------|-------------|-----|
| **Read** | Query state, list resources, subscribe to events, receive frames | Monitoring dashboards, status widgets |
| **Control** | All read + apply effects, profiles, scenes, change brightness, device config | CLI, Web UI, integrations, scripts |

Admin operations (firmware updates, factory resets) are not exposed via the API. They require direct daemon access or D-Bus with `polkit` escalation.

**Read-only API key generation:**

```
hypercolor api-key create --name "grafana-dashboard" --access read
```

**Full-access key:**

```
hypercolor api-key create --name "home-assistant" --access control
```

### 9.5 WebSocket Authentication

WebSocket connections authenticate on the initial HTTP upgrade request:

```
GET /ws HTTP/1.1
Authorization: Bearer hc_ak_x7k2m9p4q1w8...
Upgrade: websocket
```

Once upgraded, the connection inherits the authenticated session. No per-message auth.

### 9.6 CORS

When network access is enabled, CORS headers are configured to restrict cross-origin requests:

```
Access-Control-Allow-Origin: http://127.0.0.1:9420
Access-Control-Allow-Methods: GET, POST, PATCH, PUT, DELETE, OPTIONS
Access-Control-Allow-Headers: Content-Type, Authorization
```

Additional origins can be configured:

```toml
[api.cors]
allowed_origins = ["http://homeassistant.local:8123", "http://192.168.1.50:*"]
```

### 9.7 TLS

The daemon does not implement TLS directly. For network access, use a reverse proxy (Caddy, nginx) with TLS termination. This keeps the daemon simple and avoids certificate management.

```
Internet/LAN → Caddy (TLS) → 127.0.0.1:9420 (Hypercolor)
```

---

## 10. Persona Scenarios

These scenarios demonstrate how real users interact with each API surface.

### Scenario 1: Dev writes a CI build status indicator

**Persona:** Dev, writes Python scripts, has WLED strips

**Flow:** Python script polls CI status, sets lights via REST.

```python
#!/usr/bin/env python3
"""Pulse lights based on CI build status."""
import asyncio
import httpx

HYPERCOLOR = "http://127.0.0.1:9420/api/v1"

async def main():
    async with httpx.AsyncClient() as client:
        # Check latest build status (from GitHub Actions, GitLab CI, etc.)
        build = await get_build_status()

        if build.status == "failed":
            await client.post(f"{HYPERCOLOR}/effects/solid-color/apply", json={
                "controls": {
                    "color": "#ff0000",
                    "breathe": True,
                    "speed": 60
                }
            })
        elif build.status == "running":
            await client.post(f"{HYPERCOLOR}/effects/solid-color/apply", json={
                "controls": {
                    "color": "#ffaa00",
                    "breathe": True,
                    "speed": 90
                }
            })
        elif build.status == "passed":
            await client.post(f"{HYPERCOLOR}/effects/solid-color/apply", json={
                "controls": {
                    "color": "#00ff00",
                    "breathe": False
                }
            })

asyncio.run(main())
```

### Scenario 2: Streamer's OBS triggers lighting profiles

**Persona:** Luna, streams on Twitch, uses OBS + Stream Deck

**Flow:** OBS scene change → webhook → Hypercolor profile.

1. Luna configures webhook mappings in `webhooks.toml`
2. OBS Lua script fires `POST /webhooks/obs` on scene change
3. Hypercolor crossfades to the mapped profile over 1 second
4. Stream Deck shows current effect name via WebSocket state updates

```
OBS: "Starting Soon" scene active
  → POST /webhooks/obs { "event": "scene_changed", "scene_name": "Starting Soon" }
  → Hypercolor applies "stream_intro" profile (rainbow tunnel effect, 100% brightness)

OBS: switches to "Gaming" scene
  → POST /webhooks/obs { "event": "scene_changed", "scene_name": "Gaming" }
  → Hypercolor crossfades to "gaming_mode" profile (audio-pulse, cyberpunk colors)

Stream Deck "Chill Mode" button pressed
  → POST /profiles/chill/apply
  → Overrides OBS mapping until next scene change
```

### Scenario 3: Home Assistant sunset automation

**Persona:** Alex, HA power user, automated everything

**Flow:** HA sun trigger → Hypercolor REST → warm amber lighting.

```yaml
# automations.yaml
automation:
  - id: sunset_warm_rgb
    alias: "Sunset → Warm RGB"
    trigger:
      - platform: sun
        event: sunset
        offset: "-00:15:00"
    condition:
      - condition: state
        entity_id: person.alex
        state: "home"
    action:
      - service: select.select_option
        target:
          entity_id: select.hypercolor_profile
        data:
          option: "Warm Ambient"
      - service: light.turn_on
        target:
          entity_id: light.hypercolor
        data:
          brightness_pct: 60

  - id: bedtime_dim_rgb
    alias: "Bedtime → Dim & Calm"
    trigger:
      - platform: time
        at: "22:30:00"
    action:
      - service: select.select_option
        target:
          entity_id: select.hypercolor_profile
        data:
          option: "Night Mode"
      - service: light.turn_on
        target:
          entity_id: light.hypercolor
        data:
          brightness_pct: 15
```

### Scenario 4: AI-powered mood lighting via MCP

**Persona:** Bliss, uses Claude with MCP, wants Spotify-reactive lighting

**Flow:** Bliss asks Claude → Claude uses MCP tools → lighting adapts.

```
Bliss: "Make my lights match my Spotify mood — I'm listening to
        synthwave right now"

Claude's reasoning:
  1. Call get_state() → currently running "solid-color" effect at 70% brightness
  2. Call list_effects({ audio_reactive: true }) → finds "audio-pulse",
     "neon-storm", "vapor-wave", "synth-sun"
  3. Synthwave = retro, neon, purple/pink/cyan → best match: "synth-sun"
     or "vapor-wave"
  4. Call apply_effect("vapor-wave", {
       controls: { colorScheme: "Vaporwave", sensitivity: 60 },
       transition_ms: 2000
     })
  5. Call set_brightness(85)

Claude: "Done! I've applied 'Vapor Wave' with the Vaporwave color scheme
         at 85% brightness. It'll react to your music with
         retro purple and cyan tones. The 2-second crossfade should
         make the transition smooth. Want me to tweak the sensitivity
         or try a different style?"
```

### Scenario 5: Stream Deck quick-toggle

**Persona:** Jake, casual gamer, wants one-button lighting control

**Flow:** Physical button press → REST → instant profile switch.

```
Stream Deck button 1: "Gaming Mode" (configured in Stream Deck software)
  → POST /api/v1/profiles/gaming/apply
  → Audio-reactive cyberpunk lighting, 100% brightness

Stream Deck button 2: "Chill Mode"
  → POST /api/v1/profiles/chill/apply
  → Slow aurora effect, 50% brightness

Stream Deck dial: Brightness
  → PATCH /api/v1/state/brightness { "brightness": <dial_value> }
  → Real-time brightness adjustment

Stream Deck button 3: "Lights Off"
  → POST /api/v1/state/pause
  → All LEDs off instantly, one press to resume
```

---

## 11. API Versioning & Deprecation

### 11.1 Versioning Strategy

**URL-based versioning:** `/api/v1/`, `/api/v2/`, etc.

Only the REST API is versioned in the URL. Other surfaces version differently:

| Surface | Versioning Method |
|---------|------------------|
| REST | URL prefix (`/api/v1/`) |
| WebSocket | Protocol header (`hypercolor-v1`) |
| D-Bus | Bus name suffix (`Hypercolor1`) |
| MCP | Capability negotiation in `hello` |
| Unix socket | JSON-RPC method names (add new methods, don't change old ones) |

### 11.2 Compatibility Rules

**Within a major version (v1.x):**

- New endpoints can be added
- New optional fields can be added to response objects
- New optional parameters can be added to requests
- Existing endpoints, fields, and behaviors must not change
- No removals

**Breaking changes require a new major version (v2):**

- Removing or renaming endpoints
- Changing response structure
- Changing required parameters
- Changing authentication mechanism
- Changing error codes

### 11.3 Deprecation Process

1. **Announce** — Add `Deprecation` header to affected endpoints:
   ```
   Deprecation: true
   Sunset: Sat, 01 Sep 2027 00:00:00 GMT
   Link: </api/v2/effects>; rel="successor-version"
   ```

2. **Dual-run** — Both versions active for at least 6 months

3. **Warn** — Deprecated endpoints log warnings in the daemon:
   ```
   WARN  Deprecated endpoint called: GET /api/v1/effects (use /api/v2/effects)
         Client: Home-Assistant/2026.5 (192.168.1.50)
   ```

4. **Remove** — After sunset date, the old version returns `410 Gone`:
   ```json
   {
     "error": {
       "code": "gone",
       "message": "API v1 has been sunset. Please upgrade to /api/v2/. See https://docs.hypercolor.dev/migration/v1-to-v2"
     }
   }
   ```

### 11.4 Client SDK Versioning

Client SDKs (Python, TypeScript) pin to an API version. When `v2` ships, a new SDK major version is released.

```python
# hypercolor-python 1.x → targets API v1
# hypercolor-python 2.x → targets API v2
from hypercolor import HypercolorClient  # always targets the matching API version
```

### 11.5 OpenAPI Spec Versioning

Each API version maintains its own OpenAPI spec:

```
/api/v1/openapi.json  → v1 spec
/api/v2/openapi.json  → v2 spec
```

The spec includes a `x-sunset-date` extension on deprecated operations.

---

## Appendix A: Quick Reference

### REST Endpoint Summary

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/state` | Full daemon state |
| `GET` | `/state/health` | Health check |
| `GET` | `/state/metrics` | Prometheus metrics |
| `PATCH` | `/state/brightness` | Set brightness |
| `PATCH` | `/state/fps` | Set target FPS |
| `POST` | `/state/pause` | Pause rendering |
| `POST` | `/state/resume` | Resume rendering |
| `GET` | `/devices` | List devices |
| `GET` | `/devices/:id` | Device details |
| `PATCH` | `/devices/:id` | Update device |
| `DELETE` | `/devices/:id` | Remove device |
| `POST` | `/devices/discover` | Scan for devices |
| `GET` | `/devices/:id/zones` | List zones |
| `GET` | `/effects` | List effects |
| `GET` | `/effects/:id` | Effect details |
| `POST` | `/effects/:id/apply` | Apply effect |
| `GET` | `/effects/current` | Current effect |
| `PATCH` | `/effects/current/controls` | Update controls |
| `GET` | `/effects/:id/presets` | List presets |
| `POST` | `/effects/:id/presets` | Save preset |
| `POST` | `/effects/:id/presets/:name/apply` | Apply preset |
| `POST` | `/effects/next` | Next effect |
| `POST` | `/effects/previous` | Previous effect |
| `POST` | `/effects/shuffle` | Random effect |
| `GET` | `/profiles` | List profiles |
| `GET` | `/profiles/:id` | Profile details |
| `POST` | `/profiles` | Create profile |
| `PUT` | `/profiles/:id` | Update profile |
| `DELETE` | `/profiles/:id` | Delete profile |
| `POST` | `/profiles/:id/apply` | Apply profile |
| `GET` | `/layouts` | List layouts |
| `GET` | `/layouts/:id` | Layout details |
| `POST` | `/layouts` | Create layout |
| `PUT` | `/layouts/:id` | Update layout |
| `DELETE` | `/layouts/:id` | Delete layout |
| `POST` | `/layouts/:id/apply` | Apply layout |
| `GET` | `/scenes` | List scenes |
| `POST` | `/scenes` | Create scene |
| `PUT` | `/scenes/:id` | Update scene |
| `DELETE` | `/scenes/:id` | Delete scene |
| `POST` | `/scenes/:id/activate` | Trigger scene |
| `GET` | `/inputs` | List inputs |
| `PATCH` | `/inputs/:id` | Configure input |
| `POST` | `/inputs/:id/enable` | Enable input |
| `POST` | `/inputs/:id/disable` | Disable input |
| `POST` | `/bulk` | Bulk operations |
| `POST` | `/webhooks` | Register webhook |
| `POST` | `/webhooks/trigger` | Fire webhook |

### WebSocket Message Types

| Direction | Type | Format | Description |
|-----------|------|--------|-------------|
| S→C | `hello` | JSON | Initial state on connect |
| C→S | `subscribe` | JSON | Subscribe to channels |
| C→S | `unsubscribe` | JSON | Unsubscribe from channels |
| S→C | `0x01` | Binary | LED frame data |
| S→C | `0x02` | Binary | Audio spectrum data |
| S→C | `0x03` | Binary | Canvas pixel data |
| S→C | `event` | JSON | System event notification |
| S→C | `metrics` | JSON | Performance metrics |
| C→S | `command` | JSON | REST-equivalent command |
| S→C | `response` | JSON | Command response |

### MCP Tool Summary

| Tool | Description |
|------|-------------|
| `apply_effect` | Apply effect by name or description |
| `set_controls` | Adjust active effect parameters |
| `list_effects` | Browse effect catalog |
| `list_effect_controls` | See effect control schema |
| `list_devices` | Show connected devices |
| `configure_device` | Enable/disable/rename devices |
| `discover_devices` | Scan for new hardware |
| `apply_profile` | Load a saved profile |
| `create_profile` | Save current state |
| `create_scene` | Set up automated triggers |
| `set_brightness` | Global brightness control |
| `get_state` | Current system state |
| `pause_resume` | Pause/resume rendering |
| `suggest_effect` | AI-powered effect recommendation |
