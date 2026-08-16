# REST v1 compatibility matrix

This is the frozen description of what the Hypercolor daemon's `/api/v1` surface
emits **today**. Spec 76 §0 makes these shapes immutable for the duration of the
internal API unification program: canonical routes introduced by later phases
carry the corrected contracts, while every v1 path keeps serving the legacy
projection until an explicit deprecation.

The enforcing suite is `crates/hypercolor-daemon/tests/rest_v1_compat_tests.rs`.
This document and that file are edited together. A row here without a test there
is a claim, not a freeze.

**Reading this document:** it records reality, not intent. Several rows describe
behavior that is wrong on purpose (fabricated pagination, three parallel ETag
implementations, an error shape that bypasses the error envelope). Those are
marked, and each names the wave that corrects it on canonical routes.

---

## 1. Envelopes

### 1.1 Success envelope

Every enveloped route wraps its payload the same way. Built by
`ApiResponse::{ok, created, accepted}` in `src/api/envelope.rs`.

```json
{
  "data": { },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_0199c1f2-3a4b-7c5d-8e6f-0a1b2c3d4e5f",
    "timestamp": "2026-08-15T18:04:22.317Z"
  }
}
```

| Field | Type | Frozen value / grammar |
| --- | --- | --- |
| `data` | any | Route-specific payload |
| `meta.api_version` | string | Literal `"1.0"`, a string, not `"v1"` and not a number |
| `meta.request_id` | string | `req_` + UUID v7 |
| `meta.timestamp` | string | `YYYY-MM-DDTHH:MM:SS.mmmZ`, always UTC, always exactly three fractional digits, never an offset form |

The `meta` key set is exactly those three fields. Status codes in use: `200` via
`ok`, `201` via `created`, `202` via `accepted`.

### 1.2 Error envelope

```json
{
  "error": { "code": "not_found", "message": "Scene not found: …", "details": null },
  "meta": { "api_version": "1.0", "request_id": "req_…", "timestamp": "…" }
}
```

`details` has no `skip_serializing_if`, so it is **always present** and
serializes as an explicit `null` when the error carries no context. Removing the
null is a wire break.

`error.code` is a closed snake_case set, each pinned to one status:

| `error.code` | Status |
| --- | --- |
| `bad_request` | 400 |
| `unauthorized` | 401 |
| `forbidden` | 403 |
| `not_found` | 404 |
| `conflict` | 409 |
| `payload_too_large` | 413 |
| `unsupported_media_type` | 415 |
| `validation_error` | **422**, not 400 |
| `rate_limited` | 429 |
| `internal_error` | 500 |

### 1.3 Non-enveloped responses

Not everything on the wire is enveloped, and the exceptions are contract:

| Surface | Shape |
| --- | --- |
| `GET /health` | Bare probe object (§4) |
| 412 precondition failures | Bare `{error, current}` (§5.2) |
| 403 from a rejected WebSocket origin | Empty body (§5.3) |
| Binary routes (`/effects/{id}/cover`, `/assets/{id}/blob`, `/displays/{id}/preview.jpg`) | Raw bytes |
| Axum's own rejections (malformed JSON, 405, body-limit 413) | Plain text, no envelope |

---

## 2. Frozen list endpoints and the fabricated pagination block

Six list endpoints share one deliberate lie. Each returns **every** row in
`items` while reporting a `limit` of 50 and `has_more: false`, and none of them
take a query extractor, so `?offset=` and `?limit=` are silently discarded.

```json
{
  "data": {
    "items": [ ],
    "pagination": { "offset": 0, "limit": 50, "total": 2, "has_more": false }
  },
  "meta": { }
}
```

| Method | Path | Items key | `offset` | `limit` | `total` | `has_more` | Query params |
| --- | --- | --- | --- | --- | --- | --- | --- |
| GET | `/api/v1/effects` | `items` | `0` | `50` | real count | `false` | ignored |
| GET | `/api/v1/scenes` | `items` | `0` | `50` | real count **after** ephemeral scenes are filtered out | `false` | ignored |
| GET | `/api/v1/profiles` | `items` | `0` | `50` | real count | `false` | ignored |
| GET | `/api/v1/library/favorites` | `items` | `0` | `50` | real count | `false` | ignored |
| GET | `/api/v1/library/presets` | `items` | `0` | `50` | real count | `false` | ignored |
| GET | `/api/v1/library/playlists` | `items` | `0` | `50` | real count | `false` | ignored |

Consequence worth stating plainly: with more than fifty rows registered, a v1
client sees `total > limit` alongside `has_more: false`, and every row is in the
payload anyway. Spec 76 wave 3.3 fixes pagination on canonical routes only; the
block above stays exactly as written on v1.

The scene list has a second frozen quirk: the daemon's default scene is
`Ephemeral`, and the filter runs before the count, so a freshly started daemon
reports `total: 0` from `/api/v1/scenes` even though a scene is active.

The `Pagination` struct itself lives in `hypercolor-types::api::common` and is
shared by honest and fabricated callers alike, so it cannot be redefined to suit
either one:

```rust
pub struct Pagination { pub offset: usize, pub limit: usize, pub total: usize, pub has_more: bool }
```

### 2.1 Endpoints that really paginate

Four endpoints honor `offset`/`limit` and compute `has_more` from the real
total. They are frozen too, because a refactor that flattens all pagination into
one shape would break these in the opposite direction.

| Method | Path | Behavior |
| --- | --- | --- |
| GET | `/api/v1/devices` | Slices by `offset`/`limit`, `has_more = offset + limit < total` |
| GET | `/api/v1/layouts` | Same |
| GET | `/api/v1/logical-devices` | Same |
| GET | `/api/v1/attachments/templates` | Same |

### 2.2 A third pagination dialect

Two more endpoints self-describe with `limit == total` rather than the
hardcoded 50, which is a distinct shape from both groups above.

| Method | Path | Block |
| --- | --- | --- |
| GET | `/api/v1/effects/{id}/presets` | `offset: 0, limit: total, total, has_more: false` |
| GET | `/api/v1/devices/{id}/logical-devices` | `offset: 0, limit: items.len(), total: items.len(), has_more: false` |

---

## 3. Legacy paths

These paths stay routed and keep their legacy projections. Spec 76 §4.4 adds
canonical spellings (`active` for `current`, `zones` for `groups`) beside them
rather than replacing them.

| Method | Path | Request | Success body | Notes |
| --- | --- | --- | --- | --- |
| POST | `/api/v1/effects/{id}/apply` | Empty or apply options | `200`, enveloped | `{id}` accepts an effect id or name |
| PATCH | `/api/v1/effects/current/controls` | `{controls: {…}}` | `200`, enveloped `{effect, applied, rejected}` | **No `controls_version`, no ETag, and `If-Match` is not read at all**, while the `{id}` sibling has all three |
| PUT | `/api/v1/effects/current/controls/{name}/binding` | A bare `ControlBinding` object (`{sensor, sensor_min, sensor_max, target_min, target_max, deadband?, smoothing?}`), **not** wrapped in a `binding` key | `200`, enveloped | |
| POST | `/api/v1/effects/current/reset` | Empty | `200`, enveloped | |
| GET/POST | `/api/v1/scenes/{id}/groups/{group_id}/layers` | Layer spec on POST | `200`/`201`, enveloped, ETag | `group_id` is a zone id; layers keep the `groups` spelling while zone CRUD uses `/zones/` |
| PATCH | `/api/v1/scenes/{id}/groups/{group_id}/layers/order` | `{layer_ids: […]}` | `200`, enveloped, ETag | |
| PUT/DELETE | `/api/v1/scenes/{id}/groups/{group_id}/layers/{layer_id}` | Layer spec on PUT | `200`, enveloped, ETag | |
| PATCH | `/api/v1/scenes/{id}/groups/{group_id}/layers/{layer_id}/controls` | `{controls: {…}}` | `200`, enveloped, ETag | |
| GET | `/api/v1/config/get?key=…` | Key as a **query param**, not a path segment | `200`, enveloped `{key, value}` | `key` echoes the *normalized* key; a 404 message echoes the caller's raw key |
| POST | `/api/v1/config/set` | `{key, value: string, live?: bool}` | `200`, enveloped `{key, value, live, path}` | `value` is a string parsed as JSON with a fallback to a JSON string, so `"true"` becomes boolean `true` while `"hello"` stays `"hello"`. Returns `500 internal_error` when no `ConfigManager` is wired |

---

## 4. `/health`

Mounted on the **outer** router, not under `/api/v1`. There is no
`/api/v1/health` alias. Auth-exempt and suppressed from the access log.

```json
{
  "status": "healthy",
  "version": "0.3.2",
  "uptime_seconds": 0,
  "checks": { "render_loop": "idle", "device_backends": "ok", "event_bus": "idle" }
}
```

| Field | Domain |
| --- | --- |
| `status` | `healthy` \| `degraded` |
| `version` | `CARGO_PKG_VERSION` |
| `uptime_seconds` | unsigned integer |
| `checks.render_loop` | `ok` (running) \| `idle` (created or paused) \| `degraded` (stopped) |
| `checks.device_backends` | `ok` \| `idle` \| `degraded` |
| `checks.event_bus` | `ok` \| `idle` |

Status code is `200` when `status` is `healthy` and `503` otherwise. An `idle`
check still yields `healthy`; only a `degraded` check downgrades the whole probe.

---

## 5. Versioning, preconditions, and divergent error shapes

### 5.1 ETag and `If-Match`

Three independent implementations exist, one per versioned resource. All three
use a strong, quoted, bare integer: `ETag: "7"`.

| Version counter | GET routes emitting the ETag | Mutating routes reading `If-Match` |
| --- | --- | --- |
| `controls_version` | `GET /api/v1/effects/active` | `PATCH /api/v1/effects/{id}/controls` |
| `groups_revision` | `GET /api/v1/scenes/{id}/zones`, `GET /api/v1/scenes/{id}/zones/{zone_id}` | The seven zone mutators (create/update/delete zone, assign/unassign devices, update zone layout, update unassigned behavior) |
| `layers_version` | `GET /api/v1/scenes/{id}/groups/{group_id}/layers` | The five layer mutators (create, update, delete, reorder, patch controls) |

Successful mutations echo the **advanced** version in both the ETag and the body.

Frozen `If-Match` parsing quirks, identical across all three parsers:

| Header value | Behavior |
| --- | --- |
| absent | No precondition |
| `"5"` | Precondition on version 5 |
| `5` (unquoted) | Accepted, same as above, because the parser trims quotes rather than requiring them |
| `*` | **No precondition**, not "any existing resource" |
| `W/"5"` | **400** `bad_request`, because the `W/` survives the quote trim and fails the integer parse |
| non-ASCII | `400` `bad_request`, message `"If-Match header must be ASCII"` |
| anything else | `400` `bad_request`, message naming the specific counter |

Note the asymmetry that is itself frozen: `PATCH /api/v1/effects/current/controls`
takes no `HeaderMap` and therefore ignores `If-Match` entirely.

### 5.2 The 412 body: bare, envelope-free, with a top-level `current`

Every precondition failure returns this shape. It carries **no `meta` block and
no `error.code`**, and `current` sits at the top level as a sibling of `error`.
The 412 also carries an `ETag` header with the current version so a client can
rebase without a second GET.

```json
{ "error": "groups_revision mismatch", "current": 1 }
```

| Route family | `error` string | `current` |
| --- | --- | --- |
| `PATCH /api/v1/effects/{id}/controls` | `controls_version mismatch` | current `controls_version` (u64) |
| Zone mutators under `/api/v1/scenes/{id}/zones…` and `/api/v1/scenes/{id}/unassigned-behavior` | `groups_revision mismatch` | current `groups_revision` (u64) |
| Layer mutators under `/api/v1/scenes/{id}/groups/{group_id}/layers…` | `layers_version mismatch` | current `layers_version` (u64) |

Spec 76 §0 names this shape explicitly: v1 keeps the top-level `current`.

### 5.3 The empty-bodied 403

| Method | Path | Trigger | Response |
| --- | --- | --- | --- |
| GET | `/api/v1/ws` | `Origin` header present and neither loopback nor in `web.cors_origins` | `403`, **zero-length body**, no `Content-Type`, no envelope |

Every other 403 in the daemon (auth tier, CSRF, network allow-list) uses the
standard error envelope with `error.code: "forbidden"`.

Reaching this 403 requires a real connection. The `WebSocketUpgrade` extractor
runs ahead of the origin check and answers first when the request is not a
genuine upgrade: `400` for a plain GET with no upgrade headers, `426 Upgrade
Required` for upgrade headers arriving over a connection hyper cannot upgrade.
The compat test therefore serves the router on a loopback socket rather than
driving it through `tower::oneshot`.

---

## 6. What this matrix does not freeze

Named so the gaps are explicit rather than assumed covered:

- Per-route payload field lists. This matrix freezes envelopes, pagination,
  error shapes, headers, status codes, and legacy routing. Individual `data`
  payloads are pinned only where a legacy projection depends on them.
- Binary and streaming routes beyond noting that they bypass the envelope.
- The WebSocket protocol. Binary tags and byte layouts are frozen separately by
  Spec 76 wave 0.8.
- MCP tool and resource shapes.
