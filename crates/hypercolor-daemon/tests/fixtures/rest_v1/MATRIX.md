# REST wire matrix

This describes what the Hypercolor daemon's `/api/v1` surface emits. Under Spec
76 §0's lockstep doctrine these are intentionality fences, not freezes: a
deliberate shape change updates this document, the enforcing suite, and every
in-repo client in the same PR, while an unintended byte shift still fails CI.

The enforcing suite is `crates/hypercolor-daemon/tests/rest_v1_compat_tests.rs`.
This document and that file are edited together. A row here without a test there
is a claim, not a fence.

**Reading this document:** it records reality, not intent. Several rows describe
behavior that is wrong on purpose (fabricated pagination, three parallel ETag
implementations). Those are marked, and each names the wave that corrects it.
The error surface is not among them: one rendering serves every route.

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

Every error on the surface is a `DomainError` rendering itself. There is one
error factory, one body shape, and one place a status is decided.

```json
{
  "error": { "code": "not_found", "message": "scene not found: …" },
  "meta": { "api_version": "1.0", "request_id": "req_…", "timestamp": "…" }
}
```

`details` carries `skip_serializing_if = "Option::is_none"`, so the key is
**absent** when the error has no structured context, and present as an object
when it does:

```json
{
  "error": {
    "code": "precondition_failed",
    "message": "version mismatch: expected 0, current 1",
    "details": { "expected": 0, "current": 1 }
  },
  "meta": { }
}
```

`error.code` is a closed snake_case set, each pinned to one status and one
`DomainError` variant:

| `error.code` | Status | Variant | `details` |
| --- | --- | --- | --- |
| `malformed_request` | 400 | `Malformed` | none |
| `unauthorized` | 401 | `Unauthorized` | none |
| `forbidden` | 403 | `Forbidden` | optional, caller-supplied |
| `not_found` | 404 | `NotFound` | none |
| `conflict` | 409 | `Conflict` | optional, caller-supplied |
| `precondition_failed` | 412 | `PreconditionFailed` | `{expected, current}` |
| `payload_too_large` | 413 | `PayloadTooLarge` | `{limit_bytes}` |
| `unsupported_media_type` | 415 | `UnsupportedMediaType` | none |
| `validation_error` | **422**, not 400 | `Validation` | optional; `field` folds in |
| `rate_limited` | 429 | `RateLimited` | `{limit, window_seconds, retry_after}` |
| `internal_error` | 500 | `Internal` | none |
| `device_unavailable` | 503 | `DeviceUnavailable` | none |
| `service_unavailable` | 503 | `ServiceUnavailable` | optional, caller-supplied |

`device_unavailable` is the one row no route emits today. The variant is
contract (Spec 76 §2.1) and the MCP projection consumes it, but every device
refusal currently reaches the wire as a `conflict`. It is listed so the closed
set is complete, not because a client will see it.

The one structured `conflict` shape is the scene-commit compare-and-swap
loss: `details: { kind: "scene_commit_superseded", expected_revision,
current_revision }`, branchable by `kind` so a client re-reads instead of
parsing prose.

Two message rules are contract:

- **Not-found prose is derived, never hand-written.** The message is
  `"{kind} not found: {id}"` with a lowercase resource kind, so `Scene not
  found: default` is now `scene not found: default`. A route cannot invent its
  own phrasing for a miss.
- **Internal messages never reach the wire.** `internal_error` always reads
  `internal error`; the full error chain goes to `tracing` at ERROR level.

### 1.3 Non-enveloped responses

Not everything on the wire is enveloped, and the exceptions are contract:

| Surface | Shape |
| --- | --- |
| `GET /health` | Bare probe object (§4) |
| Binary routes (`/effects/{id}/cover`, `/assets/{id}/blob`, `/displays/{id}/preview.jpg`) | Raw bytes |
| Axum's own rejections (malformed JSON, 405, body-limit 413) | Plain text, no envelope |

Axum's own rejections are the only errors that bypass the envelope, because
they are answered by the framework before a handler runs. Everything the daemon
itself refuses goes out enveloped.

**An unmatched path is not one of them.** A fallback scoped to `/api/v1` and
registered inside the nest renders `404 not_found` with the canonical envelope
and the message `route not found: {path}`, echoing the caller's original path.
It exists because the web UI installs an SPA fallback on the outer router: with
a UI mounted, an unmatched API path would otherwise miss `ServeDir`, fall
through to `index.html`, and answer `200 text/html`, which would make every
route-deletion fence in the program pass while the deleted route still served a
page in production. The API fallback resolves first, so the SPA never sees an
API path, and `/api/v1/openapi.json` plus the Swagger mount keep their exact
routes. Pinned by `api_tests.rs::the_spa_fallback_never_answers_for_a_deleted_api_route`
and `::an_unmatched_api_path_renders_the_canonical_envelope_without_a_ui`; the
deletion fences in `rest_v1_compat_tests.rs::renamed_routes_leave_nothing_behind`
assert the envelope rather than the status alone for the same reason.

Paths outside `/api/v1` still belong to the SPA, which is what makes
client-side routing work; `/health` has no sub-paths to protect.

---

## 2. Frozen list endpoints and the fabricated pagination block

Six list endpoints share one deliberate lie. Each returns **every** row in
`items` while reporting a `limit` of 50 and `has_more: false`. Five take no
query extractor at all, and the sixth (`/api/v1/effects`, see below) names no
paging arguments, so `?offset=` and `?limit=` are silently discarded on every
row.

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
| GET | `/api/v1/effects` | `items` | `0` | `50` | count **after** filtering | `false` | honored (below) |
| GET | `/api/v1/scenes` | `items` | `0` | `50` | real count **after** ephemeral scenes are filtered out | `false` | ignored |
| GET | `/api/v1/profiles` | `items` | `0` | `50` | real count | `false` | ignored |
| GET | `/api/v1/library/favorites` | `items` | `0` | `50` | real count | `false` | ignored |
| GET | `/api/v1/library/presets` | `items` | `0` | `50` | real count | `false` | ignored |
| GET | `/api/v1/library/playlists` | `items` | `0` | `50` | real count | `false` | ignored |

Consequence worth stating plainly: with more than fifty rows registered, a v1
client sees `total > limit` alongside `has_more: false`, and every row is in the
payload anyway. Spec 76 wave 3.3 fixes pagination on canonical routes only; the
block above stays exactly as written on v1.

`GET /api/v1/effects` is the one row that reads its query string (Spec 78
wave 78.0a). It honors `category`, `audio_reactive`, `screen_reactive`,
`input_reactive`, `source`, and `q`, all narrowing the catalog server-side, plus
`include=controls,presets`, which adds those two optional keys to each summary.
A filter naming a value the type system does not have (`category=gaming`,
`source=wasm`, `include=everything`) answers `422 validation_error` rather than
an empty list. Summaries carry **no** `controls` or `presets` key unless
`include` asked for it, so the default shape is byte-identical to the pre-78.0
payload. Pinned by `tests/effect_catalog_tests.rs`.

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

## 3. Paths whose bodies deviate from their siblings

Every path here is canonical. Wave C1b renamed `current` to `active` and
`groups` to `zones`; wave 78.2 merged `/output/power`, `/settings/brightness`,
`/effects/pause`, and `/effects/resume` onto one `/output` resource and moved
`/audio/devices` to `/system/audio-devices`. Every one of those old spellings
was deleted outright, so nothing in this table has a second address. What
earns a row is a body or header contract that diverges from the neighbouring
routes, noted per row.

Retired paths answer 404, with one pinned exception: a POST to
`/api/v1/effects/pause` or `/api/v1/effects/resume` falls through to the
GET-only `/api/v1/effects/{id}` sibling and answers `405`. That is still a
deletion, and the 405 is what would catch someone re-adding a handler.

**One effective output state.** A destructive stop reads as stopped everywhere.
`GET /api/v1/output` reports `power: "paused"`; WS `hello`, MCP `get_status`,
and `hypercolor://state` report `running: false, paused: true`. A stop does not
need to publish a synthetic `Paused` event because every snapshot surface reads
the same effective power state directly.

| Surface | Stopped reads as | Pinned by |
| --- | --- | --- |
| `GET /output` | `power: "paused"` | `api_tests.rs::a_stopped_output_reads_as_paused_and_patches_back_to_running`, `domain/output.rs::every_dark_state_observes_as_paused` |
| WS `hello` | `running: false, paused: true` | `api/ws/tests.rs::hello_reports_a_destructive_stop_as_not_running_and_paused` |
| MCP `get_status`, `hypercolor://state` | `running: false, paused: true` | `mcp_tests.rs::mcp_status_surfaces_report_effective_session_pause` |

| Method | Path | Request | Success body | Notes |
| --- | --- | --- | --- | --- |
| GET | `/api/v1/output` | Empty | `200`, enveloped `{power, brightness}` | `power` is `running` \| `paused`; a destructive stop and a session sleep both read as `paused`. `brightness` is a float on `0.0..=1.0`, not a percentage |
| PATCH | `/api/v1/output` | `{power?, brightness?}` | `200`, enveloped whole resource | Partial: either field or both. A document setting **neither** is `422 validation_error`, not a no-op. Brightness outside `0.0..=1.0` is `422` with `details.field = "brightness"`, and it is validated **before** power moves. Unknown fields are refused by the decoder, so they arrive as an unenveloped axum rejection (§1.3) |
| POST | `/api/v1/effects/{id}/apply` | Empty or apply options | `200`, enveloped | `{id}` accepts an effect id or name |
| PATCH | `/api/v1/effects/active/controls` | `{controls: {…}}` | `200`, enveloped `{effect, applied, rejected}` | **No `controls_version`, no ETag, and `If-Match` is not read at all**, while the `{id}` sibling has all three |
| PUT | `/api/v1/effects/active/controls/{name}/binding` | A bare `ControlBinding` object (`{sensor, sensor_min, sensor_max, target_min, target_max, deadband?, smoothing?}`), **not** wrapped in a `binding` key | `200`, enveloped | |
| POST | `/api/v1/effects/active/reset` | Empty | `200`, enveloped | |
| GET/POST | `/api/v1/scenes/{id}/zones/{zone_id}/layers` | Layer spec on POST | `200`/`201`, enveloped, ETag | Layer stacks and zone CRUD share the `/zones/{zone_id}` prefix |
| PATCH | `/api/v1/scenes/{id}/zones/{zone_id}/layers/order` | `{layer_ids: […]}` | `200`, enveloped, ETag | |
| PUT/DELETE | `/api/v1/scenes/{id}/zones/{zone_id}/layers/{layer_id}` | Layer spec on PUT | `200`, enveloped, ETag | |
| PATCH | `/api/v1/scenes/{id}/zones/{zone_id}/layers/{layer_id}/controls` | `{controls: {…}}` | `200`, enveloped, ETag | |
| GET | `/api/v1/config` | Empty | `200`, enveloped whole config | Secret-classified sections render as `{redacted: true}`: every `drivers` entry, plus any top-level section the build does not model |
| GET | `/api/v1/config/keys/{key}` | Dotted key as one **path segment** | `200`, enveloped `{key, value}` | `key` echoes the *normalized* key; a 404 message echoes the caller's raw key; a malformed key (empty segment) is `400 bad_request` |
| PUT | `/api/v1/config/keys/{key}` | **The value itself** as the JSON body; `?live=` (default `true`) gates the live apply | `200`, enveloped `{key, value, live, requires_restart, pending_restart, path}` | The body is typed JSON, so `true` is boolean and `"hello"` is a string. Returns `500 internal_error` when no `ConfigManager` is wired |
| DELETE | `/api/v1/config/keys/{key}` | `?live=` (default `true`) | `200`, same mutation body, `value` carrying the restored default | |
| POST | `/api/v1/config/reset` | No body; `?live=` (default `true`) | `200`, mutation body with `key` and `value` null | Whole-config reset only; the `drivers` map, unmodeled sections, and the include list survive |
| GET | `/api/v1/config/schema` | Empty | `200`, enveloped list of `{pattern, apply, redaction, has_validator}` | The key registry as clients read it; `apply` is `{kind, section?}` |

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

## 5. Versioning and preconditions

### 5.1 ETag and `If-Match`

Three independent implementations exist, one per versioned resource. All three
use a strong, quoted, bare integer: `ETag: "7"`.

| Version counter | GET routes emitting the ETag | Mutating routes reading `If-Match` |
| --- | --- | --- |
| `controls_version` | `GET /api/v1/effects/active` | `PATCH /api/v1/effects/{id}/controls` |
| `zones_revision` | `GET /api/v1/scenes/{id}/zones`, `GET /api/v1/scenes/{id}/zones/{zone_id}` | The seven zone mutators (create/update/delete zone, assign/unassign devices, update zone layout, update unassigned behavior) |
| `layers_version` | `GET /api/v1/scenes/{id}/zones/{zone_id}/layers` | The five layer mutators (create, update, delete, reorder, patch controls) |
| `revision` (the commit generation) | Every `/api/v1/scene` read | Every structural `/api/v1/scene` write |

Successful mutations echo the **advanced** version in both the ETag and the body.

The fourth row is the Spec 78 §1.6 target shape, and it is deliberately not a
fourth counter: `revision` is the commit generation the sequencer already
assigns, so one number covers the whole live tree. The first three rows belong
to the pre-78 routes and die with them in the deletion wave; until then both
tokens are accepted, each on its own routes.

**The `/scene` tree splits its writes by kind, and the split is contractual.**
Structural writes (scene patch, zone create/patch/delete, zone layout PUT,
member assign/unassign, layer create/replace/delete/reorder, clear) honor an
optional `If-Match` against `revision`. Control-value writes
(`PATCH /api/v1/scene/zones/{zone}/layers/{layer}/controls`) honor **none** —
a guarded slider drag self-invalidates on every tick, so layer identity does
the fencing instead. A patch naming a layer that no longer exists answers 404,
never a silent write onto whatever replaced it, and a patch naming a control
key an input binding drives answers 409 `control_bound` with the bound keys in
`error.details.bound` unless the same request clears that binding.

Frozen `If-Match` parsing quirks, identical across all three parsers:

| Header value | Behavior |
| --- | --- |
| absent | No precondition |
| `"5"` | Precondition on version 5 |
| `5` (unquoted) | Accepted, same as above, because the parser trims quotes rather than requiring them |
| `*` | **No precondition**, not "any existing resource" |
| `W/"5"` | **400** `malformed_request`, because the `W/` survives the quote trim and fails the integer parse |
| non-ASCII | `400` `malformed_request`, message `"If-Match header must be ASCII"` |
| anything else | `400` `malformed_request`, message naming the specific counter |

An unreadable header value is a syntax failure, which is why it is a 400 rather
than the 422 a semantically-rejected request earns.

Note the asymmetry, which is itself pinned: `PATCH /api/v1/effects/active/controls`
takes no `HeaderMap` and therefore ignores `If-Match` entirely.

### 5.2 The 412 body

A precondition failure is the canonical envelope with `precondition_failed`, and
`details` naming both versions. The response also carries an `ETag` with the
current version, so a client can rebase without a second GET — `ETag` and
`details.current` always agree.

```json
{
  "error": {
    "code": "precondition_failed",
    "message": "version mismatch: expected 0, current 1",
    "details": { "expected": 0, "current": 1 }
  },
  "meta": { }
}
```

One rendering serves all three counters. Which counter a 412 is about is a
property of the route, not of the body.

| Route family | Counter the route guards |
| --- | --- |
| `PATCH /api/v1/effects/{id}/controls` | `controls_version` |
| Zone mutators under `/api/v1/scenes/{id}/zones…` and `/api/v1/scenes/{id}/unassigned-behavior` | `zones_revision` |
| Layer mutators under `/api/v1/scenes/{id}/zones/{zone_id}/layers…` | `layers_version` |

### 5.3 The WebSocket origin rejection

| Method | Path | Trigger | Response |
| --- | --- | --- | --- |
| GET | `/api/v1/ws` | `Origin` header present and neither loopback nor in `web.cors_origins` | `403`, canonical envelope, `error.code: "forbidden"` |

Every 403 in the daemon — auth tier, CSRF, network allow-list, and this one —
uses the same envelope.

Reaching this 403 requires a real connection. The `WebSocketUpgrade` extractor
runs ahead of the origin check and answers first when the request is not a
genuine upgrade: `400` for a plain GET with no upgrade headers, `426 Upgrade
Required` for upgrade headers arriving over a connection hyper cannot upgrade.
The test therefore serves the router on a loopback socket rather than driving it
through `tower::oneshot`.

---

## 6. What this matrix does not pin

Named so the gaps are explicit rather than assumed covered:

- Per-route payload field lists. This matrix pins envelopes, pagination,
  error shapes, headers, status codes, and legacy routing. Individual `data`
  payloads are pinned only where a projection depends on them.
- Binary and streaming routes beyond noting that they bypass the envelope.
- The WebSocket protocol. Binary tags and byte layouts are pinned separately by
  Spec 76 wave 0.8.
- MCP tool and resource shapes.
