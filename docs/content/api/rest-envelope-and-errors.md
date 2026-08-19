+++
title = "Envelope & errors"
description = "The shared REST envelope: meta fields, success and error shapes, and the full error-code-to-HTTP-status table."
weight = 20
+++

Every `/api/v1` response the daemon returns is wrapped in one of two envelopes.
Success bodies carry their payload under `data`; error bodies carry an error
detail under `error`. Both always include a `meta` block with the API version, a
correlation ID, and a generation timestamp. This page is the canonical contract
for those shapes and for the error code to HTTP status mapping. Every endpoint
in the [REST reference](@/api/rest.md) returns one of these two envelopes, so the
shapes are documented once here and linked from each route.

Success bodies come from `ApiResponse<T>` in `api/envelope.rs`. Failures come
from `DomainError` in `domain/mod.rs`, which renders itself and is the daemon's
only error rendering. There is no per-endpoint variation in either wrapper.

## Success envelope

A successful response is `{ data, meta }`. The `data` field holds the
endpoint-specific payload; the `meta` block is identical in shape across every
response.

```json
{
  "data": {
    "id": "borealis",
    "name": "Borealis",
    "category": "ambient"
  },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_0190d4c8-7e21-7c3a-9f0b-3a2e8c1d4f56",
    "timestamp": "2026-06-24T18:03:11.482Z"
  }
}
```

The daemon emits three success statuses, depending on the operation:

| Status | Builder | When |
| --- | --- | --- |
| `200 OK` | `ApiResponse::ok` | Read succeeded, or a mutation completed inline. |
| `201 Created` | `ApiResponse::created` | A new resource was created (scene, layout, preset). |
| `202 Accepted` | `ApiResponse::accepted` | Work was queued and will complete asynchronously. |

All three carry the same `{ data, meta }` body. The status line is the only
difference, so a client that only inspects the body sees a uniform shape.

## Error envelope

A failed response is `{ error, meta }`. The top-level key is `error`, not `data`,
so the presence of `data` versus `error` is the cleanest discriminator a client
can branch on.

```json
{
  "error": {
    "code": "validation_error",
    "message": "speed must be between 0.0 and 2.0",
    "details": {
      "field": "speed",
      "min": 0.0,
      "max": 2.0
    }
  },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_0190d4c8-9a13-7b88-a0f2-5e6d7c8b9a01",
    "timestamp": "2026-06-24T18:03:12.118Z"
  }
}
```

The `error` object has three fields:

| Field | Type | Notes |
| --- | --- | --- |
| `code` | string | A stable machine-readable code in `snake_case`. See the table below. |
| `message` | string | A human-readable description. Safe to surface in a UI, not meant for branching logic. |
| `details` | object | Optional structured context: the offending field, conflicting IDs, validation bounds, retry timing. The key is **omitted entirely** when the error carries no context, so treat an absent `details` and an empty one the same way. |

Branch on `code`, render `message`, and read `details` when you need the
specifics. Never parse `message` to decide control flow; it is prose and can
change.

{% callout(type="tip") %}
The discriminator is the top-level key. If the body has a `data` field you have a
success; if it has an `error` field you have a failure. You do not need to inspect
the HTTP status first, though the status will always agree with the `code`.
{% end %}

## The meta block

The `meta` block is attached to every envelope, success or error, and never
varies in shape.

| Field | Example | Meaning |
| --- | --- | --- |
| `api_version` | `"1.0"` | The contract version of the response body. This is a fixed string and is **not** the same as the `v1` URL segment. The URL path versions the routes; `api_version` versions the envelope. |
| `request_id` | `"req_0190d4c8-7e21-7c3a-9f0b-3a2e8c1d4f56"` | A per-request correlation ID, always prefixed `req_` followed by a UUID v7. It is not a bare UUID. UUID v7 is time-ordered, so request IDs sort chronologically. Log it and quote it in bug reports to trace a single request through the daemon. |
| `timestamp` | `"2026-06-24T18:03:11.482Z"` | ISO 8601 UTC with millisecond precision and a trailing `Z`. This is the response generation time, not the request arrival time. |

{% callout(type="warning") %}
`api_version` is the literal string `"1.0"`. It is not `"v1"`, not `"1"`, and not
the URL segment. Do not assert equality against the path version. The two evolve
independently.
{% end %}

## Error codes and HTTP status

Every error maps to exactly one HTTP status. The `code` string in the body and
the HTTP status line always agree, so a client can switch on either. The full set
of codes lives in the `DomainError` enum in `domain/mod.rs`, which is the
daemon's only error rendering; this is the complete mapping.

| `code` | HTTP status | Meaning |
| --- | --- | --- |
| `malformed_request` | `400 Bad Request` | The request could not be parsed: bad JSON, an unreadable header value, a path segment that is not a valid identifier. |
| `unauthorized` | `401 Unauthorized` | Missing or invalid credentials. See [auth & security](@/api/auth-and-security.md). |
| `forbidden` | `403 Forbidden` | Credentials are valid but lack the permission for this operation (for example, a read-only key attempting a write). |
| `not_found` | `404 Not Found` | The resource does not exist: unknown effect ID, scene ID, device ID. |
| `conflict` | `409 Conflict` | A state conflict: a duplicate name, an ambiguous lookup, a mutation the current state refuses. |
| `precondition_failed` | `412 Precondition Failed` | An `If-Match` version precondition failed. `details` carries `expected` and `current`, and the response repeats `current` in its `ETag`. |
| `payload_too_large` | `413 Payload Too Large` | The request body exceeds the size limit (asset and attachment uploads). |
| `unsupported_media_type` | `415 Unsupported Media Type` | The `Content-Type` is not accepted for this endpoint. |
| `validation_error` | `422 Unprocessable Entity` | The request was well-formed but semantically invalid: a value out of range, an incompatible combination of fields. |
| `rate_limited` | `429 Too Many Requests` | The request was throttled by the rate limiter. See below for retry headers. |
| `internal_error` | `500 Internal Server Error` | An unexpected daemon-side failure. The `message` is intentionally generic; the `request_id` is your key to the daemon logs. |
| `device_unavailable` | `503 Service Unavailable` | The device exists but cannot serve the request right now. |

{% callout(type="danger") %}
`validation_error` maps to **422**, not 400. A well-formed request that fails a
semantic check (a speed outside its bounds, a zone that cannot accept an output)
is a 422. 400 (`malformed_request`) is reserved for requests the daemon could not
parse at all. Clients that treat every client-side error as 400 will mis-handle
validation failures.
{% end %}

### Status flow

The same `DomainError` variant drives both the body and the response status.
There is no path where they disagree.

{% mermaid() %}
graph LR
  H[Handler] -->|DomainError::validation| C[DomainError::Validation]
  C -->|serialize| B["body.error.code = validation_error"]
  C -->|status map| S["HTTP 422"]
{% end %}

## Rate-limit responses

A `rate_limited` error (HTTP 429) carries extra signal beyond the envelope. The
`details` object reports the `limit` for the operation class, the
`window_seconds` of the rolling window, and `retry_after` (seconds until the
window resets), and a set of response headers lets a client back off precisely
instead of guessing.

```json
{
  "error": {
    "code": "rate_limited",
    "message": "Discovery operation rate limit exceeded. Retry in 12 seconds.",
    "details": {
      "limit": 2,
      "window_seconds": 60,
      "retry_after": 12
    }
  },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_0190d4c8-c0de-7a44-8b91-77aa55ee33cc",
    "timestamp": "2026-06-24T18:03:14.902Z"
  }
}
```

The accompanying headers:

| Header | Meaning |
| --- | --- |
| `Retry-After` | Seconds to wait before retrying. Mirrors `details.retry_after`. Present only when a wait is required. |
| `X-RateLimit-Limit` | The request budget for the current window. |
| `X-RateLimit-Remaining` | Requests left in the current window. |
| `X-RateLimit-Reset` | Unix epoch seconds at which the window resets. |

Prefer `Retry-After` (or the equal `details.retry_after`) for backoff timing.
Throttling is a property of specific operation classes, so a 429 on one endpoint
does not mean every endpoint is throttled.

{% callout(type="info") %}
Rate limiting follows the scale-not-nerf rule: it protects shared hardware paths
like device discovery, not ordinary reads. Loopback clients (a local CLI, TUI, or
the web UI) generally never see a 429 in normal use.
{% end %}

## Handling errors in a client

A robust client follows the same three steps regardless of language.

1. Check for the `error` key (or equivalently, a non-2xx status). If present, you
   have a failure envelope.
2. Switch on `error.code`. The string is stable; the HTTP status is its mirror.
3. Read `error.details` for structured context, and on a 429 honor `Retry-After`.
   Always capture `meta.request_id` so a failure can be correlated with the daemon
   logs.

```bash
# Inspect the envelope of any endpoint with curl + jq.
curl -s http://localhost:9420/api/v1/effects/does-not-exist \
  | jq '{status_code: .error.code, message: .error.message, request_id: .meta.request_id}'
# => { "status_code": "not_found", "message": "...", "request_id": "req_..." }
```

For the routes that return these envelopes, see the [REST API
reference](@/api/rest.md). For the real-time channel and its own framing, see the
[WebSocket protocol](@/api/websocket.md).
