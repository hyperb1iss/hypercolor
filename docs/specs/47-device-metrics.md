# 47. Device Metrics

**Status:** Implemented

## Summary

Hypercolor exposes per-device output telemetry through `GET /api/v1/devices/metrics`.
The daemon computes a shared snapshot at 2 Hz and serves the latest snapshot to every
caller so clients see identical rate calculations for the same sampling window.

## Scope

The shipped backend now covers both the shared REST snapshot and the dedicated v2
WebSocket topic.

- Queue-level cumulative counters for payload bytes and async write failures
- Centralized daemon collector that derives per-device rates from cumulative deltas
- REST endpoint for the latest shared snapshot
- Dedicated WebSocket `device_metrics` topic with its own subscription config

Transport-specific telemetry and persisted history remain out of scope.

## Data Flow

1. `hypercolor-core` tracks cumulative per-queue counters in `BackendManager`.
2. A daemon background task samples `BackendManager::device_output_statistics()` every 500 ms.
3. The collector computes delta-based `fps_actual` and `payload_bps_estimate`.
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
        "fps_actual": 60.0,
        "fps_target": 60,
        "payload_bps_estimate": 1024,
        "avg_latency_ms": 12,
        "frames_sent": 120,
        "frames_dropped": 0,
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

- `fps_actual` is derived from `frames_sent` deltas across collector samples.
- `payload_bps_estimate` is derived from payload bytes only and excludes transport overhead.
- `errors_total` is cumulative for the current queue lifetime.
- The first sample after startup or reconnect reports zero rates because no prior baseline exists.

## Sanitization

`last_error` is flattened to a single line and length-capped in v1.

- Newlines and repeated whitespace are collapsed into spaces.
- Strings longer than 240 characters are truncated with `...`.
- Structural redaction of IPs, paths, and tokens is deferred to v2.

## Deferred

- Percentile latency metrics
- Transport-specific telemetry
- Persisted historical series
- Structural `last_error` redaction
