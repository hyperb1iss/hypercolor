# 47. Device Metrics

**Status:** Implemented

## Summary

Hypercolor exposes per-device output telemetry through `GET /api/v1/devices/metrics`.
The daemon computes a shared snapshot at 2 Hz and serves the latest snapshot to every
caller so clients see identical rate calculations for the same sampling window.

## Scope

The shipped backend now covers both the shared REST snapshot and the dedicated v2
WebSocket topic.

- Queue-qualified acceptance, coalescing, and transport terminal counters
- Actor acknowledgements with completed payload bytes and transport latency
- Centralized daemon collector that derives per-device rates from cumulative deltas
- REST endpoint for the latest shared snapshot
- Dedicated WebSocket `device_metrics` topic with its own subscription config

Persisted history remains out of scope.

## Data Flow

1. `hypercolor-core` tracks cumulative per-queue counters in `BackendManager`.
2. A daemon background task samples `BackendManager::device_output_statistics()` every 500 ms.
3. The collector computes delta-based accepted, queued, and delivered rates.
4. The collector stores the result in an `ArcSwap<DeviceMetricsSnapshot>`.
5. `GET /api/v1/devices/metrics` returns the latest snapshot in the standard API envelope.
6. WebSocket clients can subscribe to `device_metrics` for the same shared snapshot without
   modifying the existing aggregate `metrics` payload.

## Endpoint

`GET /api/v1/devices/metrics`

Response body:

```json
{
  "data": {
    "taken_at_ms": 1713412345678,
    "items": [
      {
        "id": "018f5d8a-9f7b-7c6f-8d11-4b79b39ef0c9",
        "delivered_fps": 60.0,
        "accepted_fps": 61.0,
        "fps_actual": 60.0,
        "fps_target": 60,
        "payload_bps_estimate": 1024,
        "avg_latency_ms": 12,
        "avg_transport_latency_ms": 8,
        "accepted": 122,
        "transport_started": 121,
        "transport_completed": 120,
        "transport_failed": 1,
        "completed_payload_bytes": 512,
        "frames_sent": 120,
        "coalesced": 2,
        "coalesced_target_cadence": 1,
        "coalesced_backend_overrun": 1,
        "frames_dropped": 2,
        "errors_total": 1,
        "last_error": "socket timeout",
        "last_sent_ago_ms": 45
      }
    ]
  },
  "meta": {
    "api_version": "1.0",
    "request_id": "req_...",
    "timestamp": "2026-04-18T12:00:00.000Z"
  }
}
```

`taken_at_ms` lives in `data`, not `meta`, because the standard API envelope metadata shape
is fixed across the daemon.

## WebSocket

Channel: `device_metrics`

Subscription example:

```json
{
  "type": "subscribe",
  "channels": ["device_metrics"],
  "config": {
    "device_metrics": { "interval_ms": 500 }
  }
}
```

Server message:

```json
{
  "type": "device_metrics",
  "timestamp": "2026-04-18T12:00:00.000Z",
  "data": {
    "taken_at_ms": 1713412345678,
    "items": []
  }
}
```

The `device_metrics` topic streams the same shared snapshot used by REST. It does not mutate
or extend the existing aggregate `metrics` topic.

## Semantics

- `accepted` counts every frame accepted at the outer per-device queue.
- `coalesced_target_cadence` is expected latest-wins replacement while waiting for the
  configured device cadence. `coalesced_backend_overrun` means transport or its worker
  was behind.
- `transport_started`, `transport_completed`, and `transport_failed` come from the exact
  generation-qualified actor attempt. Stale acknowledgements from replaced workers are ignored.
- `delivered_fps` is derived from `transport_completed`; `accepted_fps` is derived from
  `accepted`. `fps_sent` and `fps_actual` remain v1 aliases for `delivered_fps`.
- `payload_bps_estimate` is derived only from completed payload bytes and excludes transport
  framing.
- `avg_transport_latency_ms` excludes outer queue wait. `avg_write_ms` remains its v1 alias.
- `frames_dropped`, `frames_sent`, `bytes_sent`, and `errors_total` remain compatibility
  aliases for coalesced, completed, completed payload bytes, and failed delivery counters.
- `errors_total` is cumulative for the current queue lifetime.
- The first sample after startup or reconnect reports zero rates because no prior baseline exists.

## Sanitization

`last_error` is flattened to a single line and length-capped in v1.

- Newlines and repeated whitespace are collapsed into spaces.
- Strings longer than 240 characters are truncated with `...`.
- Structural redaction of IPs, paths, and tokens is deferred to v2.

## Deferred

- Percentile latency metrics
- Persisted historical series
- Structural `last_error` redaction
