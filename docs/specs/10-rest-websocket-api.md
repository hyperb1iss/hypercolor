# 10 — REST & WebSocket API Specification

> The HTTP surface of Hypercolor: every endpoint, every schema, every byte on the wire.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Conventions](#2-conventions)
3. [Authentication](#3-authentication)
4. [CORS Configuration](#4-cors-configuration)
5. [Rate Limiting](#5-rate-limiting)
6. [Devices](#6-devices)
7. [Effects](#7-effects)
8. [Profiles](#8-profiles)
9. [Layouts](#9-layouts)
10. [Scenes](#10-scenes)
11. [Inputs](#11-inputs)
12. [System State](#12-system-state)
13. [Bulk Operations](#13-bulk-operations)
14. [WebSocket API](#14-websocket-api)
15. [OpenAPI 3.1 Considerations](#15-openapi-31-considerations)

---

## 1. Overview

Hypercolor exposes an Axum-based HTTP server on `127.0.0.1:9420` (default). The server provides:

- **REST API** at `/api/v1/*` -- resource-oriented JSON endpoints for CRUD and actions
- **WebSocket** at `/api/v1/ws` -- real-time event stream, binary frame data, and bidirectional commands
- **Static assets** at `/` -- embedded SvelteKit web UI
- **OpenAPI spec** at `/api/v1/openapi.json` and Swagger UI at `/api/v1/docs`

All REST and WebSocket traffic shares the same TCP port. The WebSocket upgrades from a standard HTTP request.

**Protocol support:** HTTP/1.1 and HTTP/2 (via Axum/Hyper). WebSocket uses RFC 6455 with `permessage-deflate` for JSON messages.

---

## 2. Conventions

### 2.1 Base URL

```
http://127.0.0.1:9420/api/v1
```

All endpoint paths in this document are relative to this base unless otherwise noted.

### 2.2 Content Type

All request and response bodies use `application/json` unless explicitly stated (binary WebSocket frames are the exception).

### 2.3 Naming

- **JSON properties:** `snake_case`
- **URL path segments:** kebab-case for static segments, `:id` for dynamic parameters
- **Query parameters:** `snake_case`

### 2.4 Response Envelope

Every successful response wraps data in a standard envelope:

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

**`meta` object schema:**

| Field         | Type     | Description                                    |
| ------------- | -------- | ---------------------------------------------- |
| `api_version` | `string` | Always `"1.0"` for v1 endpoints                |
| `request_id`  | `string` | Unique per-request identifier, prefixed `req_` |
| `timestamp`   | `string` | ISO 8601 UTC timestamp of response generation  |

### 2.5 Error Envelope

All error responses use:

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

**`error` object schema:**

| Field     | Type     | Description                                                   |
| --------- | -------- | ------------------------------------------------------------- |
| `code`    | `string` | Machine-readable error code (see table below)                 |
| `message` | `string` | Human-readable description                                    |
| `details` | `object` | Additional context (validation errors, conflicting IDs, etc.) |

### 2.6 Standard Error Codes

| HTTP Status | Code               | Meaning                                                       |
| ----------- | ------------------ | ------------------------------------------------------------- |
| 400         | `bad_request`      | Malformed request body or invalid parameters                  |
| 401         | `unauthorized`     | Missing or invalid API key (network access only)              |
| 403         | `forbidden`        | Insufficient permissions for this operation                   |
| 404         | `not_found`        | Resource does not exist                                       |
| 409         | `conflict`         | State conflict (e.g., device already connected, duplicate ID) |
| 422         | `validation_error` | Request body fails schema validation                          |
| 429         | `rate_limited`     | Too many requests (network access only)                       |
| 500         | `internal_error`   | Unexpected daemon error                                       |
| 503         | `unavailable`      | Daemon is starting up or shutting down                        |

**Validation error details example:**

```json
{
  "error": {
    "code": "validation_error",
    "message": "Request body validation failed",
    "details": {
      "fields": [
        {
          "path": "controls.effectSpeed",
          "message": "Value 150 exceeds maximum of 100",
          "constraint": "maximum"
        },
        {
          "path": "transition.duration_ms",
          "message": "Must be a positive integer",
          "constraint": "minimum"
        }
      ]
    }
  },
  "meta": { ... }
}
```

### 2.7 Pagination

List endpoints accept these query parameters:

| Parameter | Type      | Default            | Description                         |
| --------- | --------- | ------------------ | ----------------------------------- |
| `offset`  | `integer` | `0`                | Number of items to skip             |
| `limit`   | `integer` | `50`               | Maximum items to return (max: 200)  |
| `sort`    | `string`  | resource-dependent | Field to sort by                    |
| `order`   | `string`  | `"asc"`            | Sort direction: `"asc"` or `"desc"` |

Paginated responses include a `pagination` object inside `data`:

```json
{
  "data": {
    "items": [ ... ],
    "pagination": {
      "offset": 0,
      "limit": 50,
      "total": 234,
      "has_more": true
    }
  },
  "meta": { ... }
}
```

### 2.8 Filtering

List endpoints accept resource-specific filter parameters as query strings. Filters are combined with AND logic.

```
GET /api/v1/effects?audio_reactive=true&category=ambient
GET /api/v1/devices?status=connected&backend=wled
```

### 2.9 Full-Text Search

Endpoints that support search accept a `q` parameter:

```
GET /api/v1/effects?q=aurora
```

Searches across name, description, author, and tags. Results are ordered by relevance score when `q` is present (overrides `sort`).

---

## 3. Authentication

### 3.1 Local Access (No Auth)

When the daemon is bound to `127.0.0.1` or `::1` (the default), **no authentication is required**. Requests from the loopback interface and the Unix socket are always trusted.

### 3.2 Network Access (API Key)

When the daemon binds to `0.0.0.0` or any non-loopback address, all requests from non-loopback origins must include an API key.

**Header format:**

```
Authorization: Bearer hc_ak_x7k2m9p4q1w8...
```

**WebSocket authentication** occurs during the HTTP upgrade:

```
GET /api/v1/ws HTTP/1.1
Authorization: Bearer hc_ak_x7k2m9p4q1w8...
Upgrade: websocket
```

Alternatively, via query parameter (for clients that cannot set upgrade headers):

```
ws://192.168.1.100:9420/api/v1/ws?token=hc_ak_x7k2m9p4q1w8...
```

### 3.3 Access Tiers

| Tier        | Capabilities                                                             | Key Prefix |
| ----------- | ------------------------------------------------------------------------ | ---------- |
| **Read**    | Query state, list resources, subscribe to events, receive frames         | `hc_ak_r_` |
| **Control** | All read capabilities + mutations (apply effects, create profiles, etc.) | `hc_ak_`   |

A request with a read-only key that attempts a mutation receives:

```json
{
  "error": {
    "code": "forbidden",
    "message": "Read-only API key cannot perform write operations",
    "details": {
      "required_tier": "control",
      "current_tier": "read"
    }
  },
  "meta": { ... }
}
```

---

## 4. CORS Configuration

CORS headers are returned on all responses when network access is enabled.

**Default headers:**

```
Access-Control-Allow-Origin: http://127.0.0.1:9420
Access-Control-Allow-Methods: GET, POST, PATCH, PUT, DELETE, OPTIONS
Access-Control-Allow-Headers: Content-Type, Authorization
Access-Control-Max-Age: 86400
```

Preflight `OPTIONS` requests receive a `204 No Content` with the CORS headers.

Additional origins are configurable via `daemon.toml`:

```toml
[api.cors]
allowed_origins = [
  "http://homeassistant.local:8123",
  "http://192.168.1.50:*"
]
```

When the daemon is localhost-only, CORS is permissive (`*`) since cross-origin attacks against a local service have minimal risk.

---

## 5. Rate Limiting

Rate limiting applies **only** when the daemon is network-accessible (non-loopback bind address). Localhost and Unix socket access is always unlimited.

### 5.1 Rate Tiers

| Tier             | Limit       | Scope  | Applies To                       |
| ---------------- | ----------- | ------ | -------------------------------- |
| Read operations  | 120 req/min | Per IP | `GET` requests                   |
| Write operations | 60 req/min  | Per IP | `POST`, `PATCH`, `PUT`, `DELETE` |
| Bulk operations  | 10 req/min  | Per IP | `POST /api/v1/bulk`              |
| Discovery scans  | 2 req/min   | Global | `POST /api/v1/devices/discover`  |
| WebSocket frames | Unlimited   | N/A    | Binary frame data                |

### 5.2 Rate Limit Headers

Included on every response when rate limiting is active:

```
X-RateLimit-Limit: 120
X-RateLimit-Remaining: 117
X-RateLimit-Reset: 1709294460
```

| Header                  | Type      | Description                            |
| ----------------------- | --------- | -------------------------------------- |
| `X-RateLimit-Limit`     | `integer` | Maximum requests allowed in the window |
| `X-RateLimit-Remaining` | `integer` | Requests remaining in current window   |
| `X-RateLimit-Reset`     | `integer` | Unix timestamp when the window resets  |

### 5.3 Rate Limit Exceeded Response

```
HTTP/1.1 429 Too Many Requests
Retry-After: 23
X-RateLimit-Limit: 60
X-RateLimit-Remaining: 0
X-RateLimit-Reset: 1709294460
```

```json
{
  "error": {
    "code": "rate_limited",
    "message": "Write operation rate limit exceeded. Retry in 23 seconds.",
    "details": {
      "limit": 60,
      "window_seconds": 60,
      "retry_after": 23
    }
  },
  "meta": { ... }
}
```

---

## 6. Devices

### 6.1 List Devices

```
GET /api/v1/devices
```

**Query parameters:**

| Parameter | Type      | Default  | Description                                                                  |
| --------- | --------- | -------- | ---------------------------------------------------------------------------- |
| `status`  | `string`  | all      | Filter: `"connected"`, `"disconnected"`, `"error"`                           |
| `backend` | `string`  | all      | Filter by backend: `"wled"`, `"hid"`, `"hue"`, `"razer"`                     |
| `q`       | `string`  | --       | Search by name                                                               |
| `offset`  | `integer` | `0`      | Pagination offset                                                            |
| `limit`   | `integer` | `50`     | Pagination limit                                                             |
| `sort`    | `string`  | `"name"` | Sort field: `"name"`, `"status"`, `"backend"`, `"total_leds"`, `"last_seen"` |
| `order`   | `string`  | `"asc"`  | `"asc"` or `"desc"`                                                          |

**Response — `200 OK`:**

```json
{
  "data": {
    "items": [
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
    ],
    "pagination": {
      "offset": 0,
      "limit": 50,
      "total": 5,
      "has_more": false
    }
  },
  "meta": { ... }
}
```

**Device object schema:**

| Field              | Type         | Nullable | Description                                                  |
| ------------------ | ------------ | -------- | ------------------------------------------------------------ |
| `id`               | `string`     | no       | Stable device identifier                                     |
| `name`             | `string`     | no       | Display name (user-editable)                                 |
| `backend`          | `string`     | no       | Driver backend: `"wled"`, `"hid"`, `"hue"`, `"razer"`        |
| `status`           | `string`     | no       | `"connected"`, `"disconnected"`, `"error"`, `"initializing"` |
| `firmware_version` | `string`     | yes      | Firmware/driver version if known                             |
| `total_leds`       | `integer`    | no       | Sum of LEDs across all zones                                 |
| `zones`            | `Zone[]`     | no       | Array of device zones                                        |
| `connection`       | `Connection` | no       | Connection details                                           |
| `last_seen`        | `string`     | no       | ISO 8601 timestamp of last communication                     |
| `metadata`         | `object`     | no       | Backend-specific metadata                                    |

**Zone object schema:**

| Field         | Type      | Description                                             |
| ------------- | --------- | ------------------------------------------------------- |
| `id`          | `string`  | Zone identifier (unique within device)                  |
| `name`        | `string`  | Display name                                            |
| `led_count`   | `integer` | Number of LEDs in this zone                             |
| `topology`    | `string`  | `"strip"`, `"matrix"`, `"ring"`, `"single"`, `"custom"` |
| `color_order` | `string`  | `"rgb"`, `"grb"`, `"bgr"`, `"rgbw"`                     |

**Connection object schema:**

| Field     | Type      | Description                                                                |
| --------- | --------- | -------------------------------------------------------------------------- |
| `type`    | `string`  | Protocol: `"ddp"`, `"e131"`, `"artnet"`, `"usb_hid"`, `"http"`, `"serial"` |
| `address` | `string`  | IP address, USB path, or serial port                                       |
| `port`    | `integer` | Network port (if applicable)                                               |

**Error responses:**

| Status | Code          | Condition             |
| ------ | ------------- | --------------------- |
| 503    | `unavailable` | Daemon is starting up |

---

### 6.2 Get Device

```
GET /api/v1/devices/:id
```

**Path parameters:**

| Parameter | Type     | Description       |
| --------- | -------- | ----------------- |
| `id`      | `string` | Device identifier |

**Response — `200 OK`:**

Returns a single device object (same schema as list items) in the `data` field.

```json
{
  "data": {
    "id": "wled_living_room_strip",
    "name": "Living Room Strip",
    "backend": "wled",
    "status": "connected",
    "firmware_version": "0.15.3",
    "total_leds": 120,
    "zones": [ ... ],
    "connection": { ... },
    "last_seen": "2026-03-01T12:00:00Z",
    "metadata": { ... }
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code        | Condition                |
| ------ | ----------- | ------------------------ |
| 404    | `not_found` | Device ID does not exist |

---

### 6.3 Update Device

```
PATCH /api/v1/devices/:id
```

Partial update of device configuration. Only supplied fields are modified.

**Path parameters:**

| Parameter | Type     | Description       |
| --------- | -------- | ----------------- |
| `id`      | `string` | Device identifier |

**Request body:**

```json
{
  "name": "Desk Underglow",
  "enabled": true
}
```

| Field     | Type      | Required | Description               |
| --------- | --------- | -------- | ------------------------- |
| `name`    | `string`  | no       | New display name          |
| `enabled` | `boolean` | no       | Enable/disable the device |

**Response — `200 OK`:**

Returns the updated device object.

**Error responses:**

| Status | Code               | Condition                |
| ------ | ------------------ | ------------------------ |
| 404    | `not_found`        | Device ID does not exist |
| 422    | `validation_error` | Invalid field values     |

---

### 6.4 Remove Device

```
DELETE /api/v1/devices/:id
```

Removes a device from tracking. Does not factory-reset the hardware. The device can be re-discovered.

**Response — `200 OK`:**

```json
{
  "data": {
    "id": "wled_living_room_strip",
    "removed": true
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code        | Condition                |
| ------ | ----------- | ------------------------ |
| 404    | `not_found` | Device ID does not exist |

---

### 6.5 Discover Devices

```
POST /api/v1/devices/discover
```

Triggers an asynchronous scan across all configured backends. Discovery results arrive via the WebSocket `events` channel as `DeviceDiscoveryStarted`, `DeviceDiscovered`, and `DeviceDiscoveryCompleted` events.

**Request body (optional):**

```json
{
  "backends": ["wled"],
  "timeout_ms": 10000
}
```

| Field        | Type       | Required | Default     | Description                           |
| ------------ | ---------- | -------- | ----------- | ------------------------------------- |
| `backends`   | `string[]` | no       | all enabled | Limit scan to specific backends       |
| `timeout_ms` | `integer`  | no       | `10000`     | Maximum scan duration in milliseconds |

**Response — `202 Accepted`:**

```json
{
  "data": {
    "scan_id": "scan_f8e7d6c5",
    "status": "scanning",
    "backends": ["wled"],
    "timeout_ms": 10000
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code           | Condition                                    |
| ------ | -------------- | -------------------------------------------- |
| 409    | `conflict`     | A discovery scan is already in progress      |
| 429    | `rate_limited` | Discovery rate limit exceeded (2/min global) |

---

### 6.6 List Device Zones

```
GET /api/v1/devices/:id/zones
```

**Response — `200 OK`:**

```json
{
  "data": {
    "items": [
      {
        "id": "zone_0",
        "name": "Main Strip",
        "led_count": 120,
        "topology": "strip",
        "color_order": "grb"
      }
    ]
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code        | Condition                |
| ------ | ----------- | ------------------------ |
| 404    | `not_found` | Device ID does not exist |

---

### 6.7 Get Device Zone

```
GET /api/v1/devices/:id/zones/:zone_id
```

**Response — `200 OK`:**

Returns a single zone object.

**Error responses:**

| Status | Code        | Condition                     |
| ------ | ----------- | ----------------------------- |
| 404    | `not_found` | Device or zone does not exist |

---

### 6.8 Update Device Zone

```
PATCH /api/v1/devices/:id/zones/:zone_id
```

**Request body:**

```json
{
  "name": "Bottom Strip",
  "led_count": 60,
  "color_order": "rgb",
  "topology": "strip"
}
```

| Field         | Type      | Required | Description                                             |
| ------------- | --------- | -------- | ------------------------------------------------------- |
| `name`        | `string`  | no       | Zone display name                                       |
| `led_count`   | `integer` | no       | Override LED count (for manual calibration)             |
| `color_order` | `string`  | no       | `"rgb"`, `"grb"`, `"bgr"`, `"rgbw"`                     |
| `topology`    | `string`  | no       | `"strip"`, `"matrix"`, `"ring"`, `"single"`, `"custom"` |

**Response — `200 OK`:**

Returns the updated zone object.

**Error responses:**

| Status | Code               | Condition                     |
| ------ | ------------------ | ----------------------------- |
| 404    | `not_found`        | Device or zone does not exist |
| 422    | `validation_error` | Invalid field values          |

---

### 6.9 Identify Device

```
POST /api/v1/devices/:id/identify
```

Flashes the device's LEDs in a recognizable pattern so the user can identify which physical device corresponds to this ID. Useful during initial setup.

**Request body (optional):**

```json
{
  "duration_ms": 5000,
  "color": "#ff00ff"
}
```

| Field         | Type      | Required | Default     | Description       |
| ------------- | --------- | -------- | ----------- | ----------------- |
| `duration_ms` | `integer` | no       | `3000`      | How long to flash |
| `color`       | `string`  | no       | `"#ffffff"` | Flash color (hex) |

**Response — `200 OK`:**

```json
{
  "data": {
    "device_id": "wled_living_room_strip",
    "identifying": true,
    "duration_ms": 5000
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code        | Condition              |
| ------ | ----------- | ---------------------- |
| 404    | `not_found` | Device does not exist  |
| 409    | `conflict`  | Device is disconnected |

---

### 6.10 Test Device

```
POST /api/v1/devices/:id/test
```

Sends a test color frame to the device to verify connectivity and color ordering.

**Request body:**

```json
{
  "color": "#ff0000",
  "zones": ["zone_0"]
}
```

| Field   | Type       | Required | Default     | Description               |
| ------- | ---------- | -------- | ----------- | ------------------------- |
| `color` | `string`   | no       | `"#ff0000"` | Solid color to send (hex) |
| `zones` | `string[]` | no       | all zones   | Specific zones to test    |

**Response — `200 OK`:**

```json
{
  "data": {
    "device_id": "wled_living_room_strip",
    "tested_zones": ["zone_0"],
    "latency_ms": 2.3,
    "success": true
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code        | Condition              |
| ------ | ----------- | ---------------------- |
| 404    | `not_found` | Device does not exist  |
| 409    | `conflict`  | Device is disconnected |

---

## 7. Effects

### 7.1 List Effects

```
GET /api/v1/effects
```

**Query parameters:**

| Parameter        | Type      | Default  | Description                                                                                                          |
| ---------------- | --------- | -------- | -------------------------------------------------------------------------------------------------------------------- |
| `q`              | `string`  | --       | Full-text search across name, description, author, tags                                                              |
| `category`       | `string`  | all      | Filter: `"ambient"`, `"reactive"`, `"visualizer"`, `"pattern"`, `"nature"`, `"gaming"`, `"holiday"`, `"interactive"` |
| `audio_reactive` | `boolean` | --       | Filter to audio-reactive effects only                                                                                |
| `engine`         | `string`  | all      | Filter by render engine: `"servo"`, `"wgpu"`, `"native"`                                                             |
| `source`         | `string`  | all      | Filter by source: `"builtin"`, `"community"`, `"custom"`                                                             |
| `author`         | `string`  | --       | Filter by author name                                                                                                |
| `tag`            | `string`  | --       | Filter by tag (can be repeated for OR logic)                                                                         |
| `offset`         | `integer` | `0`      | Pagination offset                                                                                                    |
| `limit`          | `integer` | `50`     | Pagination limit                                                                                                     |
| `sort`           | `string`  | `"name"` | Sort field: `"name"`, `"author"`, `"category"`, `"created_at"`                                                       |
| `order`          | `string`  | `"asc"`  | `"asc"` or `"desc"`                                                                                                  |

**Response — `200 OK`:**

```json
{
  "data": {
    "items": [
      {
        "id": "aurora",
        "name": "Aurora",
        "description": "The colors of the Northern Lights illuminate your devices. v2.0",
        "author": "Hypercolor",
        "engine": "servo",
        "category": "ambient",
        "tags": ["nature", "calm", "gradient"],
        "audio_reactive": false,
        "source": "community",
        "thumbnail_url": "/api/v1/effects/aurora/thumbnail",
        "preset_count": 2
      }
    ],
    "pagination": {
      "offset": 0,
      "limit": 50,
      "total": 234,
      "has_more": true
    }
  },
  "meta": { ... }
}
```

**Effect summary object schema (list view):**

| Field            | Type       | Description                                    |
| ---------------- | ---------- | ---------------------------------------------- |
| `id`             | `string`   | Effect identifier (slug)                       |
| `name`           | `string`   | Display name                                   |
| `description`    | `string`   | Short description                              |
| `author`         | `string`   | Effect author                                  |
| `engine`         | `string`   | Render engine: `"servo"`, `"wgpu"`, `"native"` |
| `category`       | `string`   | Primary category                               |
| `tags`           | `string[]` | Descriptive tags                               |
| `audio_reactive` | `boolean`  | Whether the effect responds to audio input     |
| `source`         | `string`   | Origin: `"builtin"`, `"community"`, `"custom"` |
| `thumbnail_url`  | `string`   | Path to preview thumbnail                      |
| `preset_count`   | `integer`  | Number of available presets                    |

---

### 7.2 Get Effect

```
GET /api/v1/effects/:id
```

Returns full effect metadata including the controls schema.

**Response — `200 OK`:**

```json
{
  "data": {
    "id": "aurora",
    "name": "Aurora",
    "description": "The colors of the Northern Lights illuminate your devices. v2.0",
    "author": "Hypercolor",
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
        "step": 1,
        "default": 40,
        "value": 40
      },
      {
        "id": "amount",
        "label": "Aurora Density",
        "type": "number",
        "min": 0,
        "max": 100,
        "step": 1,
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
        "step": 1,
        "default": 50,
        "value": 50
      }
    ],
    "presets": [
      { "name": "Northern Lights", "is_default": true },
      { "name": "Deep Ocean", "is_default": false }
    ]
  },
  "meta": { ... }
}
```

**Control object schema:**

| Field     | Type       | Present When  | Description                                                   |
| --------- | ---------- | ------------- | ------------------------------------------------------------- |
| `id`      | `string`   | always        | Control identifier                                            |
| `label`   | `string`   | always        | Human-readable label                                          |
| `type`    | `string`   | always        | `"number"`, `"color"`, `"boolean"`, `"select"`, `"gradient"`  |
| `min`     | `number`   | type=`number` | Minimum value                                                 |
| `max`     | `number`   | type=`number` | Maximum value                                                 |
| `step`    | `number`   | type=`number` | Step increment                                                |
| `default` | `any`      | always        | Default value                                                 |
| `value`   | `any`      | always        | Current value (if this effect is active, reflects live state) |
| `options` | `string[]` | type=`select` | Available options for select controls                         |

**Preset object schema:**

| Field        | Type      | Description                                 |
| ------------ | --------- | ------------------------------------------- |
| `name`       | `string`  | Preset name                                 |
| `is_default` | `boolean` | Whether this is the effect's default preset |

**Error responses:**

| Status | Code        | Condition                |
| ------ | ----------- | ------------------------ |
| 404    | `not_found` | Effect ID does not exist |

---

### 7.3 Apply Effect

```
POST /api/v1/effects/:id/apply
```

Starts rendering the specified effect. If another effect is active, transitions according to the specified (or default) transition.

**Request body (optional):**

```json
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

| Field        | Type         | Required | Default                                       | Description                          |
| ------------ | ------------ | -------- | --------------------------------------------- | ------------------------------------ |
| `controls`   | `object`     | no       | effect defaults                               | Key-value pairs of control overrides |
| `transition` | `Transition` | no       | `{ "type": "crossfade", "duration_ms": 300 }` | Transition specification             |

**Transition object schema:**

| Field         | Type      | Required | Description                                              |
| ------------- | --------- | -------- | -------------------------------------------------------- |
| `type`        | `string`  | yes      | `"crossfade"`, `"cut"`, `"fade_through_black"`, `"wipe"` |
| `duration_ms` | `integer` | no       | Transition duration (0 = instant). Default: `300`        |

**Response — `200 OK`:**

```json
{
  "data": {
    "effect": {
      "id": "aurora",
      "name": "Aurora"
    },
    "applied_controls": {
      "effectSpeed": 70,
      "frontColor": "#ff00ff",
      "colorCycle": true,
      "amount": 61,
      "backColor": "#005f49",
      "cycleSpeed": 50
    },
    "transition": {
      "type": "crossfade",
      "duration_ms": 500
    }
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code               | Condition                                        |
| ------ | ------------------ | ------------------------------------------------ |
| 404    | `not_found`        | Effect ID does not exist                         |
| 422    | `validation_error` | Control value out of range or unknown control ID |

---

### 7.4 Get Current Effect

```
GET /api/v1/effects/current
```

Returns the currently active effect with its live control values.

**Response — `200 OK`:**

```json
{
  "data": {
    "id": "aurora",
    "name": "Aurora",
    "description": "The colors of the Northern Lights illuminate your devices. v2.0",
    "author": "Hypercolor",
    "engine": "servo",
    "category": "ambient",
    "tags": ["nature", "calm", "gradient"],
    "audio_reactive": false,
    "controls": [
      {
        "id": "effectSpeed",
        "label": "Animation Speed",
        "type": "number",
        "min": 0,
        "max": 100,
        "step": 1,
        "default": 40,
        "value": 70
      }
    ]
  },
  "meta": { ... }
}
```

Returns `404` with code `not_found` if no effect is currently active (daemon is paused with no prior effect).

---

### 7.5 Update Current Effect Controls

```
PATCH /api/v1/effects/current/controls
```

Updates control values on the currently active effect. Only supplied controls are modified.

**Request body:**

```json
{
  "effectSpeed": 85,
  "amount": 30
}
```

The body is a flat object of `control_id: value` pairs.

**Response — `200 OK`:**

```json
{
  "data": {
    "effect_id": "aurora",
    "updated_controls": {
      "effectSpeed": 85,
      "amount": 30
    },
    "all_controls": {
      "effectSpeed": 85,
      "amount": 30,
      "frontColor": "#ff00ff",
      "backColor": "#005f49",
      "colorCycle": true,
      "cycleSpeed": 50
    }
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code               | Condition                                |
| ------ | ------------------ | ---------------------------------------- |
| 404    | `not_found`        | No effect is currently active            |
| 422    | `validation_error` | Unknown control ID or value out of range |

---

### 7.6 List Effect Presets

```
GET /api/v1/effects/:id/presets
```

**Response — `200 OK`:**

```json
{
  "data": {
    "items": [
      {
        "name": "Northern Lights",
        "is_default": true,
        "controls": {
          "effectSpeed": 40,
          "amount": 61,
          "frontColor": "#00ffff",
          "backColor": "#005f49",
          "colorCycle": false,
          "cycleSpeed": 50
        }
      },
      {
        "name": "Deep Ocean",
        "is_default": false,
        "controls": {
          "effectSpeed": 25,
          "amount": 80,
          "frontColor": "#0044aa",
          "backColor": "#001122",
          "colorCycle": true,
          "cycleSpeed": 20
        }
      }
    ]
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code        | Condition                |
| ------ | ----------- | ------------------------ |
| 404    | `not_found` | Effect ID does not exist |

---

### 7.7 Create Preset

```
POST /api/v1/effects/:id/presets
```

Saves the current control values (or specified values) as a named preset.

**Request body:**

```json
{
  "name": "Cyberpunk Aurora",
  "controls": {
    "effectSpeed": 90,
    "frontColor": "#ff00ff",
    "backColor": "#110033",
    "colorCycle": true,
    "cycleSpeed": 70
  }
}
```

| Field      | Type     | Required | Description                                                   |
| ---------- | -------- | -------- | ------------------------------------------------------------- |
| `name`     | `string` | yes      | Preset name (unique per effect)                               |
| `controls` | `object` | no       | Control values. If omitted, captures the current live values. |

**Response — `201 Created`:**

```json
{
  "data": {
    "effect_id": "aurora",
    "preset": {
      "name": "Cyberpunk Aurora",
      "is_default": false,
      "controls": { ... }
    }
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code               | Condition                                  |
| ------ | ------------------ | ------------------------------------------ |
| 404    | `not_found`        | Effect ID does not exist                   |
| 409    | `conflict`         | Preset name already exists for this effect |
| 422    | `validation_error` | Invalid control values                     |

---

### 7.8 Update Preset

```
PATCH /api/v1/effects/:id/presets/:name
```

**Request body:**

```json
{
  "controls": {
    "effectSpeed": 95
  }
}
```

| Field      | Type     | Required | Description                   |
| ---------- | -------- | -------- | ----------------------------- |
| `controls` | `object` | no       | Partial control value updates |
| `name`     | `string` | no       | Rename the preset             |

**Response — `200 OK`:**

Returns the updated preset object.

**Error responses:**

| Status | Code               | Condition                               |
| ------ | ------------------ | --------------------------------------- |
| 404    | `not_found`        | Effect or preset does not exist         |
| 409    | `conflict`         | New name conflicts with existing preset |
| 422    | `validation_error` | Invalid control values                  |

---

### 7.9 Delete Preset

```
DELETE /api/v1/effects/:id/presets/:name
```

**Response — `200 OK`:**

```json
{
  "data": {
    "effect_id": "aurora",
    "preset_name": "Cyberpunk Aurora",
    "deleted": true
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code        | Condition                       |
| ------ | ----------- | ------------------------------- |
| 404    | `not_found` | Effect or preset does not exist |

---

### 7.10 Apply Preset

```
POST /api/v1/effects/:id/presets/:name/apply
```

Applies the effect with the preset's control values. If the effect is not currently active, it starts it.

**Request body (optional):**

```json
{
  "transition": {
    "type": "crossfade",
    "duration_ms": 500
  }
}
```

**Response — `200 OK`:**

```json
{
  "data": {
    "effect": {
      "id": "aurora",
      "name": "Aurora"
    },
    "preset": "Deep Ocean",
    "applied_controls": {
      "effectSpeed": 25,
      "amount": 80,
      "frontColor": "#0044aa",
      "backColor": "#001122",
      "colorCycle": true,
      "cycleSpeed": 20
    }
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code        | Condition                       |
| ------ | ----------- | ------------------------------- |
| 404    | `not_found` | Effect or preset does not exist |

---

### 7.11 Next Effect

```
POST /api/v1/effects/next
```

Advances to the next effect in the history stack.

**Request body:** None.

**Response — `200 OK`:**

```json
{
  "data": {
    "effect": {
      "id": "rainbow",
      "name": "Rainbow Wave"
    },
    "history_position": 3,
    "history_length": 12
  },
  "meta": { ... }
}
```

---

### 7.12 Previous Effect

```
POST /api/v1/effects/previous
```

Returns to the previous effect in the history stack.

**Response schema:** Same as Next Effect.

---

### 7.13 Shuffle Effect

```
POST /api/v1/effects/shuffle
```

Applies a random effect from the library.

**Request body (optional):**

```json
{
  "category": "ambient",
  "audio_reactive": false,
  "transition": {
    "type": "crossfade",
    "duration_ms": 1000
  }
}
```

| Field            | Type         | Required | Description                                  |
| ---------------- | ------------ | -------- | -------------------------------------------- |
| `category`       | `string`     | no       | Restrict random selection to a category      |
| `audio_reactive` | `boolean`    | no       | Restrict to audio-reactive (or non-reactive) |
| `transition`     | `Transition` | no       | Transition specification                     |

**Response — `200 OK`:**

```json
{
  "data": {
    "effect": {
      "id": "nebula",
      "name": "Cosmic Nebula"
    },
    "applied_controls": { ... }
  },
  "meta": { ... }
}
```

---

### 7.14 Get Effect Thumbnail

```
GET /api/v1/effects/:id/thumbnail
```

Returns a preview image of the effect.

**Response — `200 OK`:**

- Content-Type: `image/webp` (or `image/png`)
- Body: binary image data (256x160 preview)

**Error responses:**

| Status | Code        | Condition                                 |
| ------ | ----------- | ----------------------------------------- |
| 404    | `not_found` | Effect does not exist or has no thumbnail |

---

## 8. Profiles

### 8.1 List Profiles

```
GET /api/v1/profiles
```

**Query parameters:**

| Parameter | Type      | Default  | Description                                          |
| --------- | --------- | -------- | ---------------------------------------------------- |
| `q`       | `string`  | --       | Search by name or description                        |
| `offset`  | `integer` | `0`      | Pagination offset                                    |
| `limit`   | `integer` | `50`     | Pagination limit                                     |
| `sort`    | `string`  | `"name"` | Sort field: `"name"`, `"created_at"`, `"updated_at"` |
| `order`   | `string`  | `"asc"`  | `"asc"` or `"desc"`                                  |

**Response — `200 OK`:**

```json
{
  "data": {
    "items": [
      {
        "id": "gaming",
        "name": "Gaming Mode",
        "description": "High-energy reactive lighting for competitive gaming",
        "created_at": "2026-02-15T20:30:00Z",
        "updated_at": "2026-02-28T14:00:00Z",
        "effect": {
          "id": "audio-pulse",
          "name": "Audio Pulse"
        },
        "layout_id": "main_setup",
        "brightness": 100
      }
    ],
    "pagination": {
      "offset": 0,
      "limit": 50,
      "total": 8,
      "has_more": false
    }
  },
  "meta": { ... }
}
```

**Profile summary object schema (list view):**

| Field         | Type        | Description                                |
| ------------- | ----------- | ------------------------------------------ |
| `id`          | `string`    | Profile identifier                         |
| `name`        | `string`    | Display name                               |
| `description` | `string`    | Human-readable description                 |
| `created_at`  | `string`    | ISO 8601 creation timestamp                |
| `updated_at`  | `string`    | ISO 8601 last-modified timestamp           |
| `effect`      | `EffectRef` | `{ "id", "name" }` of the profile's effect |
| `layout_id`   | `string`    | Associated layout ID                       |
| `brightness`  | `integer`   | Global brightness (0-100)                  |

---

### 8.2 Get Profile

```
GET /api/v1/profiles/:id
```

**Response — `200 OK`:**

```json
{
  "data": {
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
  },
  "meta": { ... }
}
```

**Full profile object schema:**

| Field             | Type      | Description                                 |
| ----------------- | --------- | ------------------------------------------- |
| `id`              | `string`  | Profile identifier                          |
| `name`            | `string`  | Display name                                |
| `description`     | `string`  | Human-readable description                  |
| `created_at`      | `string`  | ISO 8601 timestamp                          |
| `updated_at`      | `string`  | ISO 8601 timestamp                          |
| `effect`          | `object`  | Effect ID + control values snapshot         |
| `effect.id`       | `string`  | Effect identifier                           |
| `effect.controls` | `object`  | Saved control values                        |
| `layout_id`       | `string`  | Layout to activate with this profile        |
| `devices`         | `object`  | Map of `device_id` to `{ "enabled": bool }` |
| `inputs`          | `object`  | Input source configuration                  |
| `inputs.audio`    | `object`  | `{ "enabled": bool, "source": string }`     |
| `inputs.screen`   | `object`  | `{ "enabled": bool }`                       |
| `brightness`      | `integer` | Global brightness (0-100)                   |

**Error responses:**

| Status | Code        | Condition                 |
| ------ | ----------- | ------------------------- |
| 404    | `not_found` | Profile ID does not exist |

---

### 8.3 Create Profile

```
POST /api/v1/profiles
```

Creates a new profile from supplied values.

**Request body:**

```json
{
  "name": "Movie Night",
  "description": "Warm ambient lighting for watching films",
  "effect": {
    "id": "candle",
    "controls": {
      "warmth": 80,
      "flicker_intensity": 20
    }
  },
  "layout_id": "main_setup",
  "devices": {
    "wled_living_room_strip": { "enabled": true },
    "hue_desk_lamp": { "enabled": true }
  },
  "inputs": {
    "audio": { "enabled": false },
    "screen": { "enabled": false }
  },
  "brightness": 40
}
```

| Field         | Type      | Required | Description                                                |
| ------------- | --------- | -------- | ---------------------------------------------------------- |
| `name`        | `string`  | yes      | Profile display name                                       |
| `description` | `string`  | no       | Description                                                |
| `effect`      | `object`  | no       | Effect + controls. If omitted, captures current effect.    |
| `layout_id`   | `string`  | no       | Layout ID. If omitted, uses current layout.                |
| `devices`     | `object`  | no       | Device enable/disable map. If omitted, uses current state. |
| `inputs`      | `object`  | no       | Input config. If omitted, uses current state.              |
| `brightness`  | `integer` | no       | 0-100. If omitted, uses current brightness.                |

**Response — `201 Created`:**

Returns the full profile object.

**Error responses:**

| Status | Code               | Condition                                                      |
| ------ | ------------------ | -------------------------------------------------------------- |
| 409    | `conflict`         | Profile name already exists                                    |
| 422    | `validation_error` | Invalid values (unknown effect, brightness out of range, etc.) |

---

### 8.4 Snapshot Profile

```
POST /api/v1/profiles/snapshot
```

Saves the current live system state as a new profile. Captures the active effect, all control values, device states, input configuration, brightness, and layout.

**Request body:**

```json
{
  "name": "Current Vibe",
  "description": "Snapshot of what's running right now"
}
```

| Field         | Type     | Required | Description  |
| ------------- | -------- | -------- | ------------ |
| `name`        | `string` | yes      | Profile name |
| `description` | `string` | no       | Description  |

**Response — `201 Created`:**

Returns the full profile object.

**Error responses:**

| Status | Code       | Condition                   |
| ------ | ---------- | --------------------------- |
| 409    | `conflict` | Profile name already exists |

---

### 8.5 Update Profile

```
PUT /api/v1/profiles/:id
```

Full replacement of a profile's data.

**Request body:** Same schema as Create Profile (all fields required except `description`).

**Response — `200 OK`:**

Returns the updated profile object.

**Error responses:**

| Status | Code               | Condition              |
| ------ | ------------------ | ---------------------- |
| 404    | `not_found`        | Profile does not exist |
| 422    | `validation_error` | Invalid values         |

---

### 8.6 Delete Profile

```
DELETE /api/v1/profiles/:id
```

**Response — `200 OK`:**

```json
{
  "data": {
    "id": "movie_night",
    "deleted": true
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code        | Condition                              |
| ------ | ----------- | -------------------------------------- |
| 404    | `not_found` | Profile does not exist                 |
| 409    | `conflict`  | Profile is referenced by active scenes |

---

### 8.7 Apply Profile

```
POST /api/v1/profiles/:id/apply
```

Applies a profile: sets the effect, controls, layout, device states, input configuration, and brightness.

**Request body (optional):**

```json
{
  "transition": {
    "type": "crossfade",
    "duration_ms": 1000
  }
}
```

**Response — `200 OK`:**

```json
{
  "data": {
    "profile": {
      "id": "gaming",
      "name": "Gaming Mode"
    },
    "applied": {
      "effect": "audio-pulse",
      "brightness": 100,
      "layout": "main_setup",
      "devices_enabled": 2,
      "devices_disabled": 1
    },
    "transition": {
      "type": "crossfade",
      "duration_ms": 1000
    }
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code        | Condition              |
| ------ | ----------- | ---------------------- |
| 404    | `not_found` | Profile does not exist |

---

### 8.8 Export Profile

```
GET /api/v1/profiles/:id/export
```

Returns the profile in a portable TOML format suitable for sharing or version control.

**Response — `200 OK`:**

- Content-Type: `application/toml`
- Content-Disposition: `attachment; filename="gaming.toml"`
- Body: TOML-encoded profile

**Error responses:**

| Status | Code        | Condition              |
| ------ | ----------- | ---------------------- |
| 404    | `not_found` | Profile does not exist |

---

## 9. Layouts

### 9.1 List Layouts

```
GET /api/v1/layouts
```

**Query parameters:**

| Parameter | Type      | Default  | Description                          |
| --------- | --------- | -------- | ------------------------------------ |
| `offset`  | `integer` | `0`      | Pagination offset                    |
| `limit`   | `integer` | `50`     | Pagination limit                     |
| `sort`    | `string`  | `"name"` | Sort field: `"name"`, `"created_at"` |
| `order`   | `string`  | `"asc"`  | `"asc"` or `"desc"`                  |

**Response — `200 OK`:**

```json
{
  "data": {
    "items": [
      {
        "id": "main_setup",
        "name": "Main Desk Setup",
        "canvas_width": 320,
        "canvas_height": 200,
        "zone_count": 5,
        "is_active": true
      }
    ],
    "pagination": {
      "offset": 0,
      "limit": 50,
      "total": 3,
      "has_more": false
    }
  },
  "meta": { ... }
}
```

---

### 9.2 Get Layout

```
GET /api/v1/layouts/:id
```

Returns full layout with all zone positions.

**Response — `200 OK`:**

```json
{
  "data": {
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
  },
  "meta": { ... }
}
```

**Layout object schema:**

| Field           | Type           | Description                            |
| --------------- | -------------- | -------------------------------------- |
| `id`            | `string`       | Layout identifier                      |
| `name`          | `string`       | Display name                           |
| `canvas_width`  | `integer`      | Canvas width in pixels (default: 320)  |
| `canvas_height` | `integer`      | Canvas height in pixels (default: 200) |
| `zones`         | `LayoutZone[]` | Zone placement array                   |

**LayoutZone object schema:**

| Field       | Type                     | Description                                             |
| ----------- | ------------------------ | ------------------------------------------------------- |
| `device_id` | `string`                 | Reference to the parent device                          |
| `zone_id`   | `string`                 | Reference to the device zone                            |
| `position`  | `{ x: float, y: float }` | Normalized position (0.0-1.0) on canvas                 |
| `size`      | `{ w: float, h: float }` | Normalized size (0.0-1.0) on canvas                     |
| `rotation`  | `float`                  | Rotation in degrees (0-360)                             |
| `topology`  | `string`                 | `"strip"`, `"matrix"`, `"ring"`, `"single"`, `"custom"` |
| `led_count` | `integer`                | Number of LEDs in this zone                             |
| `mirror`    | `boolean`                | Mirror the sampled pixels                               |
| `reverse`   | `boolean`                | Reverse LED order                                       |

**Error responses:**

| Status | Code        | Condition                |
| ------ | ----------- | ------------------------ |
| 404    | `not_found` | Layout ID does not exist |

---

### 9.3 Create Layout

```
POST /api/v1/layouts
```

**Request body:**

```json
{
  "name": "Gaming Setup v2",
  "canvas_width": 320,
  "canvas_height": 200,
  "zones": [
    {
      "device_id": "wled_living_room_strip",
      "zone_id": "zone_0",
      "position": { "x": 0.0, "y": 0.85 },
      "size": { "w": 1.0, "h": 0.15 },
      "rotation": 0.0,
      "topology": "strip",
      "led_count": 120,
      "mirror": false,
      "reverse": false
    }
  ]
}
```

| Field           | Type           | Required | Default | Description          |
| --------------- | -------------- | -------- | ------- | -------------------- |
| `name`          | `string`       | yes      | --      | Layout name          |
| `canvas_width`  | `integer`      | no       | `320`   | Canvas width         |
| `canvas_height` | `integer`      | no       | `200`   | Canvas height        |
| `zones`         | `LayoutZone[]` | yes      | --      | Zone placement array |

**Response — `201 Created`:**

Returns the full layout object.

**Error responses:**

| Status | Code               | Condition                                       |
| ------ | ------------------ | ----------------------------------------------- |
| 409    | `conflict`         | Layout name already exists                      |
| 422    | `validation_error` | Invalid zone references, positions out of range |

---

### 9.4 Update Layout

```
PUT /api/v1/layouts/:id
```

Full replacement of layout data.

**Request body:** Same schema as Create Layout.

**Response — `200 OK`:**

Returns the updated layout object.

**Error responses:**

| Status | Code               | Condition                                       |
| ------ | ------------------ | ----------------------------------------------- |
| 404    | `not_found`        | Layout does not exist                           |
| 422    | `validation_error` | Invalid zone references, positions out of range |

---

### 9.5 Delete Layout

```
DELETE /api/v1/layouts/:id
```

**Response — `200 OK`:**

```json
{
  "data": {
    "id": "gaming_setup_v2",
    "deleted": true
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code        | Condition                                                        |
| ------ | ----------- | ---------------------------------------------------------------- |
| 404    | `not_found` | Layout does not exist                                            |
| 409    | `conflict`  | Layout is the currently active layout, or referenced by profiles |

---

### 9.6 Apply Layout

```
POST /api/v1/layouts/:id/apply
```

Sets this layout as the active spatial mapping.

**Request body:** None.

**Response — `200 OK`:**

```json
{
  "data": {
    "layout": {
      "id": "main_setup",
      "name": "Main Desk Setup"
    },
    "previous_layout": {
      "id": "old_setup",
      "name": "Old Setup"
    }
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code        | Condition             |
| ------ | ----------- | --------------------- |
| 404    | `not_found` | Layout does not exist |

---

### 9.7 Get Current Layout

```
GET /api/v1/layouts/current
```

Returns the currently active layout (full layout object).

**Response — `200 OK`:**

Same schema as Get Layout.

**Error responses:**

| Status | Code        | Condition                     |
| ------ | ----------- | ----------------------------- |
| 404    | `not_found` | No layout is currently active |

---

## 10. Scenes

### 10.1 List Scenes

```
GET /api/v1/scenes
```

**Query parameters:**

| Parameter      | Type      | Default  | Description                                                                         |
| -------------- | --------- | -------- | ----------------------------------------------------------------------------------- |
| `enabled`      | `boolean` | --       | Filter by enabled/disabled state                                                    |
| `trigger_type` | `string`  | --       | Filter by trigger type: `"schedule"`, `"webhook"`, `"event"`, `"device"`, `"input"` |
| `q`            | `string`  | --       | Search by name                                                                      |
| `offset`       | `integer` | `0`      | Pagination offset                                                                   |
| `limit`        | `integer` | `50`     | Pagination limit                                                                    |
| `sort`         | `string`  | `"name"` | Sort field: `"name"`, `"created_at"`, `"enabled"`                                   |
| `order`        | `string`  | `"asc"`  | `"asc"` or `"desc"`                                                                 |

**Response — `200 OK`:**

```json
{
  "data": {
    "items": [
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
        },
        "last_triggered": "2026-02-28T17:42:00Z",
        "created_at": "2026-02-01T10:00:00Z"
      }
    ],
    "pagination": {
      "offset": 0,
      "limit": 50,
      "total": 4,
      "has_more": false
    }
  },
  "meta": { ... }
}
```

---

### 10.2 Get Scene

```
GET /api/v1/scenes/:id
```

**Response — `200 OK`:**

Returns the full scene object (same schema as list items with all nested details).

**Scene object schema:**

| Field            | Type           | Description                                          |
| ---------------- | -------------- | ---------------------------------------------------- |
| `id`             | `string`       | Scene identifier                                     |
| `name`           | `string`       | Display name                                         |
| `enabled`        | `boolean`      | Whether the scene is actively listening for triggers |
| `profile_id`     | `string`       | Profile to apply when triggered                      |
| `trigger`        | `Trigger`      | Trigger configuration                                |
| `conditions`     | `Condition[]`  | Additional conditions that must be met               |
| `transition`     | `Transition`   | How to transition when activating                    |
| `last_triggered` | `string\|null` | ISO 8601 timestamp of last activation                |
| `created_at`     | `string`       | ISO 8601 creation timestamp                          |
| `updated_at`     | `string`       | ISO 8601 last-modified timestamp                     |

**Trigger object schema:**

| Field        | Type       | Present When    | Description                                                 |
| ------------ | ---------- | --------------- | ----------------------------------------------------------- |
| `type`       | `string`   | always          | `"schedule"`, `"webhook"`, `"event"`, `"device"`, `"input"` |
| `schedule`   | `Schedule` | type=`schedule` | Schedule specification                                      |
| `secret`     | `string`   | type=`webhook`  | Webhook authentication secret                               |
| `event_type` | `string`   | type=`event`    | Internal event type to react to                             |
| `filter`     | `object`   | type=`event`    | Event data filter                                           |
| `device_id`  | `string`   | type=`device`   | Device to watch                                             |
| `state`      | `string`   | type=`device`   | `"connected"` or `"disconnected"`                           |
| `source`     | `string`   | type=`input`    | Input source: `"audio"`, `"screen"`, `"keyboard"`           |
| `threshold`  | `float`    | type=`input`    | Activation threshold (0.0-1.0)                              |

**Schedule object schema:**

| Field            | Type      | Description                                   |
| ---------------- | --------- | --------------------------------------------- |
| `type`           | `string`  | `"cron"` or `"solar"`                         |
| `cron`           | `string`  | Cron expression (when type=`cron`)            |
| `event`          | `string`  | `"sunrise"` or `"sunset"` (when type=`solar`) |
| `offset_minutes` | `integer` | Offset from solar event in minutes            |

**Condition object schema:**

| Field       | Type       | Description                                       |
| ----------- | ---------- | ------------------------------------------------- |
| `type`      | `string`   | `"time_range"`, `"day_of_week"`, `"device_state"` |
| `after`     | `string`   | Start time `"HH:MM"` (when type=`time_range`)     |
| `before`    | `string`   | End time `"HH:MM"` (when type=`time_range`)       |
| `days`      | `string[]` | Days of week (when type=`day_of_week`)            |
| `device_id` | `string`   | Device ID (when type=`device_state`)              |
| `state`     | `string`   | Required state (when type=`device_state`)         |

**Error responses:**

| Status | Code        | Condition               |
| ------ | ----------- | ----------------------- |
| 404    | `not_found` | Scene ID does not exist |

---

### 10.3 Create Scene

```
POST /api/v1/scenes
```

**Request body:**

```json
{
  "name": "Gaming Time",
  "profile_id": "gaming",
  "trigger": {
    "type": "event",
    "event_type": "DeviceConnected",
    "filter": {
      "device_id": "razer_huntsman"
    }
  },
  "conditions": [
    {
      "type": "time_range",
      "after": "18:00",
      "before": "02:00"
    }
  ],
  "transition": {
    "type": "crossfade",
    "duration_ms": 500
  },
  "enabled": true
}
```

| Field        | Type          | Required | Default                                        | Description                 |
| ------------ | ------------- | -------- | ---------------------------------------------- | --------------------------- |
| `name`       | `string`      | yes      | --                                             | Scene name                  |
| `profile_id` | `string`      | yes      | --                                             | Profile to apply            |
| `trigger`    | `Trigger`     | yes      | --                                             | Trigger configuration       |
| `conditions` | `Condition[]` | no       | `[]`                                           | Additional conditions       |
| `transition` | `Transition`  | no       | `{ "type": "crossfade", "duration_ms": 1000 }` | Transition spec             |
| `enabled`    | `boolean`     | no       | `true`                                         | Whether the scene is active |

**Response — `201 Created`:**

Returns the full scene object.

**Error responses:**

| Status | Code               | Condition                                        |
| ------ | ------------------ | ------------------------------------------------ |
| 404    | `not_found`        | Referenced profile_id does not exist             |
| 422    | `validation_error` | Invalid trigger, condition, or transition values |

---

### 10.4 Update Scene

```
PUT /api/v1/scenes/:id
```

Full replacement of scene data.

**Request body:** Same schema as Create Scene.

**Response — `200 OK`:**

Returns the updated scene object.

**Error responses:**

| Status | Code               | Condition            |
| ------ | ------------------ | -------------------- |
| 404    | `not_found`        | Scene does not exist |
| 422    | `validation_error` | Invalid values       |

---

### 10.5 Delete Scene

```
DELETE /api/v1/scenes/:id
```

**Response — `200 OK`:**

```json
{
  "data": {
    "id": "gaming_time",
    "deleted": true
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code        | Condition            |
| ------ | ----------- | -------------------- |
| 404    | `not_found` | Scene does not exist |

---

### 10.6 Activate Scene

```
POST /api/v1/scenes/:id/activate
```

Manually triggers a scene, applying its profile with its configured transition. Ignores trigger conditions.

**Request body:** None.

**Response — `200 OK`:**

```json
{
  "data": {
    "scene": {
      "id": "sunset_warm",
      "name": "Sunset Warmth"
    },
    "profile_applied": {
      "id": "warm_ambient",
      "name": "Warm Ambient"
    },
    "transition": {
      "type": "crossfade",
      "duration_ms": 3000
    }
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code        | Condition            |
| ------ | ----------- | -------------------- |
| 404    | `not_found` | Scene does not exist |

---

### 10.7 Enable/Disable Scene

```
PATCH /api/v1/scenes/:id/enabled
```

**Request body:**

```json
{
  "enabled": false
}
```

**Response — `200 OK`:**

```json
{
  "data": {
    "id": "sunset_warm",
    "enabled": false
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code        | Condition            |
| ------ | ----------- | -------------------- |
| 404    | `not_found` | Scene does not exist |

---

## 11. Inputs

### 11.1 List Inputs

```
GET /api/v1/inputs
```

Returns all available input sources (audio capture, screen capture, keyboard).

**Response — `200 OK`:**

```json
{
  "data": {
    "items": [
      {
        "id": "audio_default",
        "type": "audio",
        "name": "System Audio",
        "enabled": true,
        "status": "active",
        "device_name": "PipeWire Multimedia (default)"
      },
      {
        "id": "screen_primary",
        "type": "screen",
        "name": "Primary Monitor",
        "enabled": false,
        "status": "disabled",
        "device_name": "DP-1 (3440x1440)"
      },
      {
        "id": "keyboard_default",
        "type": "keyboard",
        "name": "System Keyboard",
        "enabled": false,
        "status": "disabled",
        "device_name": "Razer Huntsman V2 (/dev/input/event4)"
      }
    ]
  },
  "meta": { ... }
}
```

**Input summary object schema:**

| Field         | Type      | Description                                           |
| ------------- | --------- | ----------------------------------------------------- |
| `id`          | `string`  | Input source identifier                               |
| `type`        | `string`  | `"audio"`, `"screen"`, `"keyboard"`                   |
| `name`        | `string`  | Display name                                          |
| `enabled`     | `boolean` | Whether this input is active                          |
| `status`      | `string`  | `"active"`, `"disabled"`, `"error"`, `"initializing"` |
| `device_name` | `string`  | Underlying hardware/system device name                |

---

### 11.2 Get Input

```
GET /api/v1/inputs/:id
```

Returns full input details including configuration and live levels.

**Response — `200 OK` (audio input):**

```json
{
  "data": {
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
  },
  "meta": { ... }
}
```

**Response — `200 OK` (screen input):**

```json
{
  "data": {
    "id": "screen_primary",
    "type": "screen",
    "name": "Primary Monitor",
    "enabled": false,
    "status": "disabled",
    "device_name": "DP-1 (3440x1440)",
    "config": {
      "capture_method": "pipewire",
      "target_fps": 30,
      "region": null,
      "downsample_factor": 4
    }
  },
  "meta": { ... }
}
```

**Audio config object schema:**

| Field              | Type           | Description                                            |
| ------------------ | -------------- | ------------------------------------------------------ |
| `fft_size`         | `integer`      | FFT window size (power of 2: 512, 1024, 2048, 4096)    |
| `smoothing`        | `float`        | Temporal smoothing factor (0.0-1.0, higher = smoother) |
| `noise_gate`       | `float`        | Minimum level threshold (0.0-1.0)                      |
| `frequency_range`  | `{ min, max }` | Frequency range in Hz                                  |
| `gain`             | `float`        | Input gain multiplier (0.1-10.0)                       |
| `beat_sensitivity` | `float`        | Beat detection sensitivity (0.0-1.0)                   |

**Audio levels object schema:**

| Field             | Type      | Description                           |
| ----------------- | --------- | ------------------------------------- |
| `level`           | `float`   | Overall RMS level (0.0-1.0)           |
| `bass`            | `float`   | Low frequency energy (0.0-1.0)        |
| `mid`             | `float`   | Mid frequency energy (0.0-1.0)        |
| `treble`          | `float`   | High frequency energy (0.0-1.0)       |
| `beat`            | `boolean` | Whether a beat is detected this frame |
| `beat_confidence` | `float`   | Beat detection confidence (0.0-1.0)   |

**Error responses:**

| Status | Code        | Condition               |
| ------ | ----------- | ----------------------- |
| 404    | `not_found` | Input ID does not exist |

---

### 11.3 Configure Input

```
PATCH /api/v1/inputs/:id
```

Updates input source configuration. Only supplied fields are modified.

**Request body (audio example):**

```json
{
  "config": {
    "smoothing": 0.8,
    "gain": 1.5,
    "beat_sensitivity": 0.7
  }
}
```

**Request body (screen example):**

```json
{
  "config": {
    "target_fps": 15,
    "region": { "x": 0, "y": 0, "width": 1920, "height": 1080 }
  }
}
```

**Response — `200 OK`:**

Returns the updated input object.

**Error responses:**

| Status | Code               | Condition             |
| ------ | ------------------ | --------------------- |
| 404    | `not_found`        | Input does not exist  |
| 422    | `validation_error` | Invalid config values |

---

### 11.4 Enable Input

```
POST /api/v1/inputs/:id/enable
```

Starts the input source (begins audio capture, screen capture, or keyboard monitoring).

**Request body:** None.

**Response — `200 OK`:**

```json
{
  "data": {
    "id": "audio_default",
    "enabled": true,
    "status": "active"
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code             | Condition                         |
| ------ | ---------------- | --------------------------------- |
| 404    | `not_found`      | Input does not exist              |
| 409    | `conflict`       | Input is already enabled          |
| 500    | `internal_error` | Failed to initialize input source |

---

### 11.5 Disable Input

```
POST /api/v1/inputs/:id/disable
```

Stops the input source.

**Request body:** None.

**Response — `200 OK`:**

```json
{
  "data": {
    "id": "audio_default",
    "enabled": false,
    "status": "disabled"
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code        | Condition            |
| ------ | ----------- | -------------------- |
| 404    | `not_found` | Input does not exist |

---

### 11.6 Get Audio Spectrum

```
GET /api/v1/inputs/audio/spectrum
```

Returns the current audio spectrum snapshot. For real-time spectrum data, use the WebSocket `spectrum` channel instead.

**Response — `200 OK`:**

```json
{
  "data": {
    "timestamp": "2026-03-01T12:00:00.123Z",
    "levels": {
      "level": 0.42,
      "bass": 0.71,
      "mid": 0.35,
      "treble": 0.18,
      "beat": false,
      "beat_confidence": 0.45
    },
    "bins": [0.12, 0.34, 0.56, 0.71, 0.65, 0.43, 0.31, 0.22],
    "bin_count": 64,
    "frequency_range": { "min": 20, "max": 20000 },
    "bpm": 128.4
  },
  "meta": { ... }
}
```

| Field             | Type           | Description                              |
| ----------------- | -------------- | ---------------------------------------- |
| `timestamp`       | `string`       | ISO 8601 timestamp of the snapshot       |
| `levels`          | `AudioLevels`  | Aggregated frequency band levels         |
| `bins`            | `float[]`      | FFT magnitude bins (mel-scaled, 0.0-1.0) |
| `bin_count`       | `integer`      | Number of frequency bins                 |
| `frequency_range` | `{ min, max }` | Frequency range in Hz                    |
| `bpm`             | `float\|null`  | Estimated BPM if beat tracking is active |

**Error responses:**

| Status | Code       | Condition                  |
| ------ | ---------- | -------------------------- |
| 409    | `conflict` | Audio input is not enabled |

---

### 11.7 Get Audio Config

```
GET /api/v1/inputs/audio/config
```

Shorthand for `GET /api/v1/inputs/audio_default` -- returns the audio analysis configuration.

**Response — `200 OK`:**

```json
{
  "data": {
    "fft_size": 2048,
    "smoothing": 0.7,
    "noise_gate": 0.02,
    "frequency_range": { "min": 20, "max": 20000 },
    "gain": 1.0,
    "beat_sensitivity": 0.6
  },
  "meta": { ... }
}
```

---

### 11.8 Update Audio Config

```
PATCH /api/v1/inputs/audio/config
```

Shorthand for `PATCH /api/v1/inputs/audio_default`.

**Request body:**

```json
{
  "smoothing": 0.8,
  "beat_sensitivity": 0.75,
  "gain": 1.2
}
```

**Response — `200 OK`:**

Returns the full audio config object.

**Error responses:**

| Status | Code               | Condition           |
| ------ | ------------------ | ------------------- |
| 422    | `validation_error` | Values out of range |

---

## 12. System State

### 12.1 Get State

```
GET /api/v1/state
```

Returns a full snapshot of the daemon's current state.

**Response — `200 OK`:**

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
      "total": 7,
      "total_leds": 842
    },
    "inputs": {
      "audio": "active",
      "screen": "disabled",
      "keyboard": "disabled"
    },
    "uptime_seconds": 86423,
    "version": "0.1.0"
  },
  "meta": { ... }
}
```

**State object schema:**

| Field                | Type               | Description                                        |
| -------------------- | ------------------ | -------------------------------------------------- |
| `running`            | `boolean`          | Whether the daemon is fully initialized            |
| `paused`             | `boolean`          | Whether rendering is paused                        |
| `brightness`         | `integer`          | Global brightness (0-100)                          |
| `fps.target`         | `integer`          | Target frames per second                           |
| `fps.actual`         | `float`            | Measured FPS over the last second                  |
| `effect`             | `EffectRef\|null`  | Currently active effect (`{ "id", "name" }`)       |
| `profile`            | `ProfileRef\|null` | Currently active profile (`{ "id", "name" }`)      |
| `layout`             | `LayoutRef\|null`  | Currently active layout (`{ "id", "name" }`)       |
| `devices.connected`  | `integer`          | Number of connected devices                        |
| `devices.total`      | `integer`          | Number of known devices (connected + disconnected) |
| `devices.total_leds` | `integer`          | Sum of LEDs across connected devices               |
| `inputs.audio`       | `string`           | `"active"`, `"disabled"`, `"error"`                |
| `inputs.screen`      | `string`           | `"active"`, `"disabled"`, `"error"`                |
| `inputs.keyboard`    | `string`           | `"active"`, `"disabled"`, `"error"`                |
| `uptime_seconds`     | `integer`          | Seconds since daemon started                       |
| `version`            | `string`           | Daemon version string                              |

---

### 12.2 Health Check

```
GET /api/v1/state/health
```

Lightweight health check for load balancers, monitoring systems, and Home Assistant availability checks.

**Response — `200 OK` (healthy):**

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

**Response — `503 Service Unavailable` (unhealthy):**

```json
{
  "status": "unhealthy",
  "version": "0.1.0",
  "uptime_seconds": 86423,
  "checks": {
    "render_loop": "ok",
    "device_backends": "degraded",
    "event_bus": "ok"
  }
}
```

Note: The health endpoint does **not** use the standard response envelope -- it returns a flat object for compatibility with common health check tools.

**Check status values:** `"ok"`, `"degraded"`, `"failed"`

---

### 12.3 Metrics

```
GET /api/v1/state/metrics
```

Returns Prometheus-compatible metrics.

**Response — `200 OK`:**

- Content-Type: `text/plain; version=0.0.4; charset=utf-8`

```
# HELP hypercolor_fps_actual Current measured frames per second
# TYPE hypercolor_fps_actual gauge
hypercolor_fps_actual 59.7

# HELP hypercolor_frame_time_ms Frame render time in milliseconds
# TYPE hypercolor_frame_time_ms histogram
hypercolor_frame_time_ms_bucket{le="1"} 0
hypercolor_frame_time_ms_bucket{le="2"} 12
hypercolor_frame_time_ms_bucket{le="4"} 89
hypercolor_frame_time_ms_bucket{le="8"} 156
hypercolor_frame_time_ms_bucket{le="16"} 160
hypercolor_frame_time_ms_bucket{le="+Inf"} 160
hypercolor_frame_time_ms_sum 540.2
hypercolor_frame_time_ms_count 160

# HELP hypercolor_devices_connected Number of connected devices
# TYPE hypercolor_devices_connected gauge
hypercolor_devices_connected 5

# HELP hypercolor_leds_total Total number of active LEDs
# TYPE hypercolor_leds_total gauge
hypercolor_leds_total 842

# HELP hypercolor_brightness Global brightness level
# TYPE hypercolor_brightness gauge
hypercolor_brightness 85

# HELP hypercolor_uptime_seconds Daemon uptime in seconds
# TYPE hypercolor_uptime_seconds counter
hypercolor_uptime_seconds 86423
```

---

### 12.4 Set Brightness

```
PATCH /api/v1/state/brightness
```

**Request body:**

```json
{
  "brightness": 75
}
```

| Field        | Type      | Required | Description               |
| ------------ | --------- | -------- | ------------------------- |
| `brightness` | `integer` | yes      | Global brightness (0-100) |

**Response — `200 OK`:**

```json
{
  "data": {
    "brightness": 75,
    "previous": 85
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code               | Condition                     |
| ------ | ------------------ | ----------------------------- |
| 422    | `validation_error` | Brightness out of 0-100 range |

---

### 12.5 Set Target FPS

```
PATCH /api/v1/state/fps
```

**Request body:**

```json
{
  "fps": 30
}
```

| Field | Type      | Required | Description        |
| ----- | --------- | -------- | ------------------ |
| `fps` | `integer` | yes      | Target FPS (1-120) |

**Response — `200 OK`:**

```json
{
  "data": {
    "fps": {
      "target": 30,
      "actual": 29.9
    },
    "previous_target": 60
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code               | Condition              |
| ------ | ------------------ | ---------------------- |
| 422    | `validation_error` | FPS out of 1-120 range |

---

### 12.6 Pause Rendering

```
POST /api/v1/state/pause
```

Pauses the render loop. All LEDs are set to black (off). Device connections remain active.

**Request body:** None.

**Response — `200 OK`:**

```json
{
  "data": {
    "paused": true
  },
  "meta": { ... }
}
```

---

### 12.7 Resume Rendering

```
POST /api/v1/state/resume
```

Resumes rendering from where it left off. The previously active effect continues.

**Request body:** None.

**Response — `200 OK`:**

```json
{
  "data": {
    "paused": false,
    "effect": {
      "id": "aurora",
      "name": "Aurora"
    }
  },
  "meta": { ... }
}
```

---

## 13. Bulk Operations

### 13.1 Execute Bulk Operations

```
POST /api/v1/bulk
```

Execute multiple API operations in a single request. Operations can optionally be atomic (all-or-nothing).

**Request body:**

```json
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
      "body": {
        "controls": { "effectSpeed": 60 },
        "transition": { "type": "crossfade", "duration_ms": 500 }
      }
    },
    {
      "method": "PATCH",
      "path": "/state/brightness",
      "body": { "brightness": 80 }
    }
  ],
  "atomic": true
}
```

**Operation object schema:**

| Field    | Type     | Required | Description                                                    |
| -------- | -------- | -------- | -------------------------------------------------------------- |
| `method` | `string` | yes      | HTTP method: `"GET"`, `"POST"`, `"PATCH"`, `"PUT"`, `"DELETE"` |
| `path`   | `string` | yes      | API path relative to `/api/v1` (without the prefix)            |
| `body`   | `object` | no       | Request body (for POST/PATCH/PUT)                              |

**Top-level fields:**

| Field        | Type          | Required | Default | Description                                                 |
| ------------ | ------------- | -------- | ------- | ----------------------------------------------------------- |
| `operations` | `Operation[]` | yes      | --      | Ordered list of operations (max: 20)                        |
| `atomic`     | `boolean`     | no       | `false` | If true, all operations must succeed or all are rolled back |

**Response — `200 OK`:**

```json
{
  "data": {
    "results": [
      {
        "index": 0,
        "status": 200,
        "data": { "id": "wled_strip_1", "name": "Strip 1", "enabled": true }
      },
      {
        "index": 1,
        "status": 200,
        "data": { "id": "wled_strip_2", "name": "Strip 2", "enabled": true }
      },
      {
        "index": 2,
        "status": 200,
        "data": { "effect": { "id": "aurora", "name": "Aurora" } }
      },
      {
        "index": 3,
        "status": 200,
        "data": { "brightness": 80, "previous": 85 }
      }
    ],
    "all_succeeded": true
  },
  "meta": { ... }
}
```

**Partial failure response (atomic=false):**

```json
{
  "data": {
    "results": [
      { "index": 0, "status": 200, "data": { ... } },
      {
        "index": 1,
        "status": 404,
        "error": {
          "code": "not_found",
          "message": "Device 'wled_strip_2' does not exist"
        }
      },
      { "index": 2, "status": 200, "data": { ... } },
      { "index": 3, "status": 200, "data": { ... } }
    ],
    "all_succeeded": false
  },
  "meta": { ... }
}
```

**Atomic failure response (atomic=true):**

When `atomic: true` and any operation fails, all operations are rolled back:

```json
{
  "data": {
    "results": [
      { "index": 0, "status": 200, "data": { ... }, "rolled_back": true },
      {
        "index": 1,
        "status": 404,
        "error": {
          "code": "not_found",
          "message": "Device 'wled_strip_2' does not exist"
        }
      }
    ],
    "all_succeeded": false,
    "rolled_back": true,
    "failed_at_index": 1
  },
  "meta": { ... }
}
```

**Error responses:**

| Status | Code               | Condition                                  |
| ------ | ------------------ | ------------------------------------------ |
| 400    | `bad_request`      | Empty operations array or exceeds max (20) |
| 422    | `validation_error` | Invalid operation method or path format    |
| 429    | `rate_limited`     | Bulk rate limit exceeded (10/min)          |

---

## 14. WebSocket API

### 14.1 Connection

**Endpoint:** `ws://127.0.0.1:9420/api/v1/ws`

**Upgrade request:**

```
GET /api/v1/ws HTTP/1.1
Host: 127.0.0.1:9420
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Version: 13
Sec-WebSocket-Protocol: hypercolor-v1
```

For network access with authentication:

```
GET /api/v1/ws HTTP/1.1
Host: 192.168.1.100:9420
Authorization: Bearer hc_ak_x7k2m9p4q1w8...
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Version: 13
Sec-WebSocket-Protocol: hypercolor-v1
```

Or via query parameter:

```
GET /api/v1/ws?token=hc_ak_x7k2m9p4q1w8... HTTP/1.1
```

**Compression:** `permessage-deflate` is enabled for JSON messages. Binary messages are sent uncompressed (already compact; compression adds latency).

---

### 14.2 Hello Message

On connection, the server immediately sends a `hello` message with a full state snapshot:

```json
{
  "type": "hello",
  "version": "1.0",
  "state": {
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
    "device_count": 5,
    "total_leds": 842
  },
  "capabilities": [
    "frames",
    "spectrum",
    "events",
    "commands",
    "canvas",
    "metrics"
  ],
  "subscriptions": ["events"]
}
```

**Hello message schema:**

| Field           | Type       | Description                                                             |
| --------------- | ---------- | ----------------------------------------------------------------------- |
| `type`          | `string`   | Always `"hello"`                                                        |
| `version`       | `string`   | WebSocket protocol version (`"1.0"`)                                    |
| `state`         | `object`   | Current daemon state snapshot                                           |
| `capabilities`  | `string[]` | Channels this server supports                                           |
| `subscriptions` | `string[]` | Channels this client is initially subscribed to (default: `["events"]`) |

---

### 14.3 Channel Subscription

Clients control bandwidth by subscribing to specific channels. By default, only `events` is subscribed.

**Subscribe message (client -> server):**

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

**Subscribe message schema:**

| Field      | Type       | Required | Description               |
| ---------- | ---------- | -------- | ------------------------- |
| `type`     | `string`   | yes      | `"subscribe"`             |
| `channels` | `string[]` | yes      | Channels to subscribe to  |
| `config`   | `object`   | no       | Per-channel configuration |

**Channel configuration options:**

| Channel    | Config Field  | Type       | Default    | Description                                         |
| ---------- | ------------- | ---------- | ---------- | --------------------------------------------------- |
| `frames`   | `fps`         | `integer`  | `30`       | Frame delivery rate (1-60)                          |
| `frames`   | `format`      | `string`   | `"binary"` | `"binary"` (compact) or `"json"` (debug)            |
| `frames`   | `zones`       | `string[]` | `["all"]`  | Specific zone IDs or `["all"]`                      |
| `spectrum` | `fps`         | `integer`  | `30`       | Spectrum delivery rate (1-60)                       |
| `spectrum` | `bins`        | `integer`  | `64`       | Number of frequency bins (8, 16, 32, 64, 128)       |
| `canvas`   | `fps`         | `integer`  | `15`       | Canvas delivery rate (1-30)                         |
| `canvas`   | `format`      | `string`   | `"rgb"`    | `"rgb"` (3 bytes/pixel) or `"rgba"` (4 bytes/pixel) |
| `metrics`  | `interval_ms` | `integer`  | `1000`     | Metrics push interval (100-10000)                   |

**Subscribe acknowledgment (server -> client):**

```json
{
  "type": "subscribed",
  "channels": ["frames", "spectrum", "events"],
  "config": {
    "frames": { "fps": 30, "format": "binary", "zones": ["all"] },
    "spectrum": { "fps": 30, "bins": 64 }
  }
}
```

**Unsubscribe message (client -> server):**

```json
{
  "type": "unsubscribe",
  "channels": ["canvas"]
}
```

**Unsubscribe acknowledgment (server -> client):**

```json
{
  "type": "unsubscribed",
  "channels": ["canvas"],
  "remaining": ["frames", "spectrum", "events"]
}
```

### 14.4 Available Channels

| Channel    | Data Type | Default FPS | Description                                     |
| ---------- | --------- | ----------- | ----------------------------------------------- |
| `frames`   | Binary    | 30          | LED color data for all (or selected) zones      |
| `spectrum` | Binary    | 30          | Audio FFT spectrum data                         |
| `events`   | JSON      | N/A (push)  | System events (device, effect, profile changes) |
| `canvas`   | Binary    | 15          | Raw 320x200 canvas pixels (for UI preview)      |
| `metrics`  | JSON      | 1 Hz        | Performance metrics (FPS, latency, memory)      |

---

### 14.5 Binary Frame Format

Binary WebSocket messages begin with a 1-byte type discriminator.

#### Frame Message (type `0x01`)

LED color data for all subscribed zones.

```
Byte 0:         0x01 (frame type)
Bytes 1-4:      frame_number (u32 LE)
Bytes 5-8:      timestamp_ms (u32 LE) -- milliseconds since daemon start
Byte 9:         zone_count (u8)

For each zone:
  Bytes 0-1:    zone_id_length (u16 LE)
  Bytes 2-N:    zone_id (UTF-8 string)
  Bytes N+1-N+2: led_count (u16 LE)
  Bytes N+3-...: RGB triplets (led_count * 3 bytes)
```

**Bandwidth estimate:** For 842 LEDs across 5 zones: ~`9 + (5 * ~16) + (842 * 3) = 2,615 bytes` per frame. At 30 fps: **~78 KB/s**.

#### Spectrum Message (type `0x02`)

Audio FFT spectrum data.

```
Byte 0:         0x02 (spectrum type)
Bytes 1-4:      timestamp_ms (u32 LE)
Byte 5:         bin_count (u8) -- number of frequency bins
Bytes 6-9:      level (f32 LE) -- overall RMS level
Bytes 10-13:    bass (f32 LE)
Bytes 14-17:    mid (f32 LE)
Bytes 18-21:    treble (f32 LE)
Byte 22:        beat (u8, 0 or 1)
Bytes 23-26:    beat_confidence (f32 LE)
Bytes 27-...:   bins (bin_count * f32 LE)
```

**Bandwidth estimate:** With 64 bins: `27 + 256 = 283 bytes` per message. At 30 fps: **~8.5 KB/s**.

#### Canvas Message (type `0x03`)

Raw canvas pixel data (for the spatial editor preview).

```
Byte 0:         0x03 (canvas type)
Bytes 1-4:      frame_number (u32 LE)
Bytes 5-8:      timestamp_ms (u32 LE)
Bytes 9-10:     width (u16 LE) -- 320
Bytes 11-12:    height (u16 LE) -- 200
Byte 13:        format (u8) -- 0 = RGB, 1 = RGBA
Bytes 14-...:   pixel data (width * height * bpp)
```

**Bandwidth estimate:** Full canvas at RGB: `14 + 192,000 = 192,014 bytes`. At 15 fps: **~2.8 MB/s**. Only subscribe when the spatial editor is open.

---

### 14.6 JSON Event Messages

Events are pushed to clients subscribed to the `events` channel. All events use a consistent envelope:

```json
{
  "type": "event",
  "event": "effect_changed",
  "timestamp": "2026-03-01T12:00:00.123Z",
  "data": {
    "previous": { "id": "rainbow", "name": "Rainbow" },
    "current": { "id": "aurora", "name": "Aurora" },
    "trigger": "user"
  }
}
```

**Event message schema:**

| Field       | Type     | Description                                   |
| ----------- | -------- | --------------------------------------------- |
| `type`      | `string` | Always `"event"`                              |
| `event`     | `string` | Event type identifier                         |
| `timestamp` | `string` | ISO 8601 timestamp with millisecond precision |
| `data`      | `object` | Event-specific payload                        |

**Event types:**

| Event                        | Data Fields                                         | Description                                         |
| ---------------------------- | --------------------------------------------------- | --------------------------------------------------- |
| `effect_changed`             | `previous`, `current`, `trigger`                    | Active effect changed                               |
| `effect_control_changed`     | `effect_id`, `control_id`, `old_value`, `new_value` | Control value updated                               |
| `device_connected`           | `device_id`, `name`, `backend`, `led_count`         | Device came online                                  |
| `device_disconnected`        | `device_id`, `reason`                               | Device went offline                                 |
| `device_discovery_started`   | `backends`                                          | Discovery scan began                                |
| `device_discovery_completed` | `found`, `duration_ms`                              | Discovery scan finished                             |
| `device_error`               | `device_id`, `error`, `recoverable`                 | Device communication error                          |
| `profile_applied`            | `profile_id`, `profile_name`, `trigger`             | Profile activated                                   |
| `profile_saved`              | `profile_id`, `profile_name`                        | New profile created/updated                         |
| `profile_deleted`            | `profile_id`                                        | Profile removed                                     |
| `scene_triggered`            | `scene_id`, `scene_name`, `trigger_type`            | Scene automation fired                              |
| `scene_enabled`              | `scene_id`, `enabled`                               | Scene toggled on/off                                |
| `layout_changed`             | `previous`, `current`                               | Active layout changed                               |
| `layout_updated`             | `layout_id`                                         | Layout zones modified                               |
| `input_source_changed`       | `input_id`, `input_type`, `enabled`                 | Input enabled/disabled                              |
| `audio_beat`                 | `confidence`, `bpm`                                 | Beat detected                                       |
| `brightness_changed`         | `old`, `new`                                        | Global brightness changed                           |
| `fps_changed`                | `target`                                            | Target FPS changed                                  |
| `paused`                     | --                                                  | Rendering paused                                    |
| `resumed`                    | --                                                  | Rendering resumed                                   |
| `daemon_started`             | `version`, `device_count`                           | Daemon initialized                                  |
| `daemon_shutdown`            | `reason`                                            | Daemon shutting down                                |
| `error`                      | `code`, `message`, `severity`                       | System error (`"warning"`, `"error"`, `"critical"`) |
| `webhook_received`           | `webhook_id`, `source`                              | External webhook fired                              |

---

### 14.7 Metrics Messages

Pushed to clients subscribed to the `metrics` channel:

```json
{
  "type": "metrics",
  "timestamp": "2026-03-01T12:00:01.000Z",
  "data": {
    "fps": {
      "target": 60,
      "actual": 59.7,
      "dropped": 0
    },
    "frame_time": {
      "avg_ms": 4.2,
      "p95_ms": 6.8,
      "p99_ms": 9.1,
      "max_ms": 11.3
    },
    "stages": {
      "input_sampling_ms": 0.8,
      "effect_rendering_ms": 2.1,
      "spatial_sampling_ms": 0.3,
      "device_output_ms": 1.0,
      "event_bus_ms": 0.05
    },
    "memory": {
      "daemon_rss_mb": 42.5,
      "servo_rss_mb": 128.3,
      "canvas_buffer_kb": 256
    },
    "devices": {
      "connected": 5,
      "total_leds": 842,
      "output_errors": 0
    },
    "websocket": {
      "client_count": 2,
      "bytes_sent_per_sec": 85400
    }
  }
}
```

---

### 14.8 Bidirectional Commands

Clients can send REST-equivalent commands over the WebSocket connection, avoiding the overhead of separate HTTP requests.

**Command message (client -> server):**

```json
{
  "type": "command",
  "id": "cmd_001",
  "method": "POST",
  "path": "/effects/aurora/apply",
  "body": {
    "controls": { "effectSpeed": 70 },
    "transition": { "type": "crossfade", "duration_ms": 500 }
  }
}
```

**Command message schema:**

| Field    | Type     | Required | Description                                                    |
| -------- | -------- | -------- | -------------------------------------------------------------- |
| `type`   | `string` | yes      | `"command"`                                                    |
| `id`     | `string` | yes      | Client-generated correlation ID                                |
| `method` | `string` | yes      | HTTP method: `"GET"`, `"POST"`, `"PATCH"`, `"PUT"`, `"DELETE"` |
| `path`   | `string` | yes      | API path relative to `/api/v1`                                 |
| `body`   | `object` | no       | Request body                                                   |

**Response message (server -> client):**

```json
{
  "type": "response",
  "id": "cmd_001",
  "status": 200,
  "data": {
    "effect": { "id": "aurora", "name": "Aurora" },
    "applied_controls": { ... }
  }
}
```

**Error response:**

```json
{
  "type": "response",
  "id": "cmd_002",
  "status": 404,
  "error": {
    "code": "not_found",
    "message": "Effect 'nonexistent' does not exist",
    "details": {}
  }
}
```

**Response message schema:**

| Field    | Type      | Description                 |
| -------- | --------- | --------------------------- |
| `type`   | `string`  | Always `"response"`         |
| `id`     | `string`  | Matches the command's `id`  |
| `status` | `integer` | HTTP-equivalent status code |
| `data`   | `object`  | Response data (on success)  |
| `error`  | `object`  | Error object (on failure)   |

---

### 14.9 Backpressure Handling

The server manages backpressure to prevent slow clients from causing memory issues or blocking the render loop.

**Server-side behavior:**

1. Each client has a bounded send buffer (configurable, default: 64 messages).
2. If the buffer fills, the **oldest undelivered binary messages** (frames, spectrum, canvas) are dropped. JSON messages (events, responses) are never dropped.
3. When frames are dropped, the server sends a `backpressure` notification:

```json
{
  "type": "backpressure",
  "dropped_frames": 12,
  "channel": "frames",
  "recommendation": "reduce_fps",
  "suggested_fps": 15
}
```

4. If a client is consistently too slow (>50% frame drop rate over 10 seconds), the server downgrades the subscription FPS automatically and notifies:

```json
{
  "type": "subscription_downgraded",
  "channel": "frames",
  "previous_fps": 30,
  "new_fps": 15,
  "reason": "client_too_slow"
}
```

**Client-side strategy:**

- Monitor `backpressure` messages and reduce subscription FPS proactively.
- Use the `canvas` channel at 15 fps (not 30 or 60) -- it is ~2.8 MB/s even at 15.
- Only subscribe to `canvas` when the spatial editor is visible.
- Unsubscribe from `frames` when the frame preview is not in view.

---

### 14.10 Reconnection

When a WebSocket connection drops, the client should reconnect with exponential backoff:

```
Attempt 1:  immediate
Attempt 2:  500ms delay
Attempt 3:  1000ms delay
Attempt 4+: exponential backoff, max 30 seconds
```

On reconnection, the server sends a fresh `hello` message with the current state. There is no message replay -- the `hello` provides a complete state snapshot. For frame data, the client simply resumes receiving from the current frame.

After reconnecting, re-send the `subscribe` message to restore channel subscriptions.

---

### 14.11 Ping/Pong

The server sends WebSocket ping frames every 30 seconds. Clients must respond with pong frames (handled automatically by most WebSocket libraries). If no pong is received within 10 seconds, the server closes the connection.

Clients may also send ping frames; the server will respond with pong.

---

### 14.12 WebSocket Message Summary

| Direction | Type                      | Format | Description                 |
| --------- | ------------------------- | ------ | --------------------------- |
| S -> C    | `hello`                   | JSON   | Initial state on connect    |
| C -> S    | `subscribe`               | JSON   | Subscribe to channels       |
| S -> C    | `subscribed`              | JSON   | Subscription acknowledgment |
| C -> S    | `unsubscribe`             | JSON   | Unsubscribe from channels   |
| S -> C    | `unsubscribed`            | JSON   | Unsubscribe acknowledgment  |
| S -> C    | `0x01`                    | Binary | LED frame data              |
| S -> C    | `0x02`                    | Binary | Audio spectrum data         |
| S -> C    | `0x03`                    | Binary | Canvas pixel data           |
| S -> C    | `event`                   | JSON   | System event notification   |
| S -> C    | `metrics`                 | JSON   | Performance metrics         |
| C -> S    | `command`                 | JSON   | REST-equivalent command     |
| S -> C    | `response`                | JSON   | Command response            |
| S -> C    | `backpressure`            | JSON   | Backpressure warning        |
| S -> C    | `subscription_downgraded` | JSON   | Auto-downgrade notification |

---

## 15. OpenAPI 3.1 Considerations

### 15.1 Spec Location

```
GET /api/v1/openapi.json    -- OpenAPI 3.1 JSON spec
GET /api/v1/docs            -- Swagger UI (interactive)
```

### 15.2 Generation

The spec is generated at compile time from Rust types using the `utoipa` crate with `utoipa-swagger-ui` for the Axum integration. Every request/response struct derives `ToSchema`, and every handler is annotated with `#[utoipa::path(...)]`.

```rust
#[derive(ToSchema, Serialize, Deserialize)]
pub struct DeviceResponse {
    pub id: String,
    pub name: String,
    pub backend: Backend,
    pub status: DeviceStatus,
    pub total_leds: u32,
    pub zones: Vec<ZoneInfo>,
    pub connection: ConnectionInfo,
    pub last_seen: String,
    pub metadata: serde_json::Value,
}

#[utoipa::path(
    get,
    path = "/api/v1/devices/{id}",
    params(("id" = String, Path, description = "Device identifier")),
    responses(
        (status = 200, description = "Device details", body = ApiResponse<DeviceResponse>),
        (status = 404, description = "Device not found", body = ApiError),
    ),
    tag = "devices"
)]
async fn get_device(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse { ... }
```

### 15.3 Spec Structure

The OpenAPI spec uses:

- **Tags** for grouping: `devices`, `effects`, `profiles`, `layouts`, `scenes`, `inputs`, `state`, `bulk`
- **Components/schemas** for all shared types (`Device`, `Effect`, `Profile`, `Layout`, `Scene`, `InputSource`, `Transition`, `ControlValue`, `ApiResponse`, `ApiError`, `PaginationMeta`)
- **Security schemes**: `bearerAuth` (API key in `Authorization` header)
- **Servers**: `http://127.0.0.1:9420/api/v1` (local), configurable for remote

### 15.4 Versioning

Each API version maintains its own spec:

```
/api/v1/openapi.json  -- v1 spec
/api/v2/openapi.json  -- v2 spec (future)
```

Deprecated operations include the `x-sunset-date` extension:

```json
{
  "x-sunset-date": "2027-09-01T00:00:00Z",
  "deprecated": true
}
```

### 15.5 Response Envelope in OpenAPI

The generic envelope is modeled using OpenAPI 3.1's generic composition:

```yaml
ApiResponse:
  type: object
  required: [data, meta]
  properties:
    data:
      description: Response payload (varies per endpoint)
    meta:
      $ref: "#/components/schemas/ResponseMeta"

ApiError:
  type: object
  required: [error, meta]
  properties:
    error:
      $ref: "#/components/schemas/ErrorDetail"
    meta:
      $ref: "#/components/schemas/ResponseMeta"

ResponseMeta:
  type: object
  required: [api_version, request_id, timestamp]
  properties:
    api_version:
      type: string
      example: "1.0"
    request_id:
      type: string
      example: "req_a1b2c3d4"
    timestamp:
      type: string
      format: date-time

ErrorDetail:
  type: object
  required: [code, message]
  properties:
    code:
      type: string
      enum:
        [
          bad_request,
          unauthorized,
          forbidden,
          not_found,
          conflict,
          validation_error,
          rate_limited,
          internal_error,
          unavailable,
        ]
    message:
      type: string
    details:
      type: object
      additionalProperties: true

PaginatedResponse:
  type: object
  required: [items, pagination]
  properties:
    items:
      type: array
    pagination:
      $ref: "#/components/schemas/PaginationMeta"

PaginationMeta:
  type: object
  required: [offset, limit, total, has_more]
  properties:
    offset:
      type: integer
    limit:
      type: integer
    total:
      type: integer
    has_more:
      type: boolean
```

---

## Appendix A: Complete Endpoint Reference

| Method           | Path                               | Description                     |
| ---------------- | ---------------------------------- | ------------------------------- |
| **Devices**      |                                    |                                 |
| `GET`            | `/devices`                         | List all devices                |
| `GET`            | `/devices/:id`                     | Get device details              |
| `PATCH`          | `/devices/:id`                     | Update device config            |
| `DELETE`         | `/devices/:id`                     | Remove device                   |
| `POST`           | `/devices/discover`                | Trigger discovery scan          |
| `GET`            | `/devices/:id/zones`               | List device zones               |
| `GET`            | `/devices/:id/zones/:zone_id`      | Get zone details                |
| `PATCH`          | `/devices/:id/zones/:zone_id`      | Update zone config              |
| `POST`           | `/devices/:id/identify`            | Flash device for identification |
| `POST`           | `/devices/:id/test`                | Send test color frame           |
| **Effects**      |                                    |                                 |
| `GET`            | `/effects`                         | List effects                    |
| `GET`            | `/effects/:id`                     | Get effect details + controls   |
| `POST`           | `/effects/:id/apply`               | Apply effect                    |
| `GET`            | `/effects/current`                 | Get current effect              |
| `PATCH`          | `/effects/current/controls`        | Update active controls          |
| `GET`            | `/effects/:id/presets`             | List presets                    |
| `POST`           | `/effects/:id/presets`             | Create preset                   |
| `PATCH`          | `/effects/:id/presets/:name`       | Update preset                   |
| `DELETE`         | `/effects/:id/presets/:name`       | Delete preset                   |
| `POST`           | `/effects/:id/presets/:name/apply` | Apply preset                    |
| `POST`           | `/effects/next`                    | Next in history                 |
| `POST`           | `/effects/previous`                | Previous in history             |
| `POST`           | `/effects/shuffle`                 | Random effect                   |
| `GET`            | `/effects/:id/thumbnail`           | Get effect thumbnail            |
| **Profiles**     |                                    |                                 |
| `GET`            | `/profiles`                        | List profiles                   |
| `GET`            | `/profiles/:id`                    | Get profile details             |
| `POST`           | `/profiles`                        | Create profile                  |
| `PUT`            | `/profiles/:id`                    | Update profile                  |
| `DELETE`         | `/profiles/:id`                    | Delete profile                  |
| `POST`           | `/profiles/:id/apply`              | Apply profile                   |
| `POST`           | `/profiles/snapshot`               | Save current state as profile   |
| `GET`            | `/profiles/:id/export`             | Export profile as TOML          |
| **Layouts**      |                                    |                                 |
| `GET`            | `/layouts`                         | List layouts                    |
| `GET`            | `/layouts/:id`                     | Get layout details              |
| `POST`           | `/layouts`                         | Create layout                   |
| `PUT`            | `/layouts/:id`                     | Update layout                   |
| `DELETE`         | `/layouts/:id`                     | Delete layout                   |
| `POST`           | `/layouts/:id/apply`               | Set as active layout            |
| `GET`            | `/layouts/current`                 | Get current layout              |
| **Scenes**       |                                    |                                 |
| `GET`            | `/scenes`                          | List scenes                     |
| `GET`            | `/scenes/:id`                      | Get scene details               |
| `POST`           | `/scenes`                          | Create scene                    |
| `PUT`            | `/scenes/:id`                      | Update scene                    |
| `DELETE`         | `/scenes/:id`                      | Delete scene                    |
| `POST`           | `/scenes/:id/activate`             | Manually trigger scene          |
| `PATCH`          | `/scenes/:id/enabled`              | Enable/disable scene            |
| **Inputs**       |                                    |                                 |
| `GET`            | `/inputs`                          | List input sources              |
| `GET`            | `/inputs/:id`                      | Get input details + status      |
| `PATCH`          | `/inputs/:id`                      | Configure input                 |
| `POST`           | `/inputs/:id/enable`               | Enable input                    |
| `POST`           | `/inputs/:id/disable`              | Disable input                   |
| `GET`            | `/inputs/audio/spectrum`           | Get audio spectrum snapshot     |
| `GET`            | `/inputs/audio/config`             | Get audio analysis config       |
| `PATCH`          | `/inputs/audio/config`             | Update audio analysis config    |
| **System State** |                                    |                                 |
| `GET`            | `/state`                           | Full state snapshot             |
| `GET`            | `/state/health`                    | Health check                    |
| `GET`            | `/state/metrics`                   | Prometheus metrics              |
| `PATCH`          | `/state/brightness`                | Set global brightness           |
| `PATCH`          | `/state/fps`                       | Set target FPS                  |
| `POST`           | `/state/pause`                     | Pause rendering                 |
| `POST`           | `/state/resume`                    | Resume rendering                |
| **Bulk**         |                                    |                                 |
| `POST`           | `/bulk`                            | Execute multiple operations     |
| **OpenAPI**      |                                    |                                 |
| `GET`            | `/openapi.json`                    | OpenAPI 3.1 spec                |
| `GET`            | `/docs`                            | Swagger UI                      |
