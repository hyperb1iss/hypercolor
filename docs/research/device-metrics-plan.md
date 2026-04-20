# Device Performance Metrics — Implementation Plan

**Status:** Revised draft (post-codex review)
**Date:** 2026-04-18
**Scope:** Surface per-device performance telemetry via REST API and the devices page UI.

---

## Summary

Today the daemon collects rich per-device write telemetry in `DeviceOutputStatistics`
(`crates/hypercolor-core/src/device/manager.rs:316-340`) but exposes **only aggregates**
on the wire. The devices page UI has **no performance visualization**.

v1 ships a REST-only, polling-based path:

1. Extend `DeviceOutputStatistics` with `bytes_sent` and `errors_total` totals.
2. Run a centralized 2Hz collector task in the daemon that snapshots all devices into an
   `ArcSwap<DeviceMetricsSnapshot>` so every client sees identical numbers.
3. Add `GET /api/v1/devices/metrics` that reads the swap.
4. Devices page polls at 2Hz, builds sparklines client-side, renders a metric strip on
   each connected card.

WebSocket streaming is **deferred to v2** — polling at 2Hz is sufficient for this UI, and
the current WS `metrics` topic has protocol constraints that would force an invasive
refactor for v1.

---

## Revision Notes (what changed from draft 1)

- **Dropped WS integration from v1.** Codex correctly flagged that `metrics.devices` is
  already a `MetricsDevices` struct (`protocol.rs:702`), so adding `Vec<DeviceMetrics>`
  would be a type collision. The WS config floor is 100ms (`protocol.rs:269-275`), so our
  proposed 20Hz/50ms was out of range anyway. Every UI page subscribes to `metrics` by
  default at 500ms (`connection.rs:170-176`), so bolting per-device arrays in amplifies
  payload everywhere, not just on the devices page. If/when WS is added in v2, it gets its
  own `device_metrics` topic.
- **Centralized delta computation** via a single collector task writing to `ArcSwap`.
  `relay_metrics` runs per-client (`relays.rs:901`), so if each client computed its own
  rates from `DeviceOutputStatistics` deltas, two clients would see different numbers and
  reconnects would skew first samples.
- **Lean `DeviceMetrics` shape** — `id` only, no `name`/`status` duplication with
  `DeviceSummary`. UI joins against the existing devices list.
- **Renamed `bandwidth_bps` → `payload_bps_estimate`** to head off "why is this wrong?"
  questions. Docs will say explicitly: payload bytes only, excludes transport overhead.
- **Error-string sanitization** added as explicit v1 concern (length cap + category).
- **UI state model simplified** — single `RwSignal<HashMap<DeviceId, _>>` replaced
  wholesale per poll, plus memos per card. No per-device signals, no deep-compare churn.
- **Ring buffer cleanup on device removal** added to UI provider.

---

## Motivation

Device performance is invisible to users. If a WLED over DDP is dropping frames, or a Hue
bridge is degrading, nothing surfaces in the UI — users only notice when lights look wrong.
The devices page is the natural home for this telemetry because that's where users land
when they suspect device trouble.

The plumbing exists: the render loop already tracks latency, frames sent, queue wait, and
errors per device. This is wire-through work.

---

## Existing State

### What's collected per device

`DeviceOutputStatistics` (`crates/hypercolor-core/src/device/manager.rs:316-340`):

- `frames_received: u64`, `frames_sent: u64`, `frames_dropped: u64`
- `avg_latency_ms: u64`, `avg_queue_wait_ms: u64`, `avg_write_ms: u64`
- `last_error: Option<String>`, `last_sent_ago_ms: Option<u64>`
- `target_fps: u32`, `device_id`, `backend_id`

Health status (`HealthStatus::Healthy | Degraded | Unreachable`) from
`crates/hypercolor-core/src/device/traits.rs`.

### What's exposed (aggregates only)

- `GET /api/v1/status` → FPS, frame-time percentiles, aggregate `output_errors`
- `GET /api/v1/devices` → `DeviceSummary` (name, backend, LEDs, status) — no perf
- `GET /api/v1/devices/debug/queues` → dumps `DeviceOutputStatistics`, nothing consumes it
- WebSocket `metrics` topic (`daemon/src/api/ws/protocol.rs:691-704`) →
  aggregate `MetricsDevices { connected, total_leds, output_errors: u32 }`

### What the devices page shows today

Just static/status: name, backend, LED count, connection dot. No charts.

### What exists to build on

- `perf_charts::Sparkline` (`crates/hypercolor-ui/src/components/perf_charts.rs:13-128`) —
  SVG polyline + area fill, already used on the dashboard
- SilkCircuit palette for status coloring
- Existing polling patterns in the UI (verify pattern before writing new)

---

## Success Criteria

1. `GET /api/v1/devices/metrics` returns per-device FPS, payload bandwidth estimate,
   error totals, avg latency — with rates computed identically for every caller
2. Devices page polls at 2Hz and renders, on every connected card:
   - Live numeric readout: `60/60 fps · 1.2 MB/s · 0 errors`
   - Last-error tooltip on hover when errors are present
   - FPS sparkline (60-sample rolling window)
3. No changes to existing `MetricsPayload` shape, no changes to WS subscription defaults
4. `just verify` passes
5. Device removal (unplug, forget) prunes metric state and sparkline buffers within one
   poll cycle

---

## Constraints

- `DeviceSummary` shape stays backward-compatible. Per-device metrics are served from a
  dedicated endpoint so the existing devices list response does not grow
- WS `MetricsPayload` is **not touched** in v1. v2 can add a dedicated `device_metrics`
  topic if polling proves insufficient
- UI crate is excluded from the Cargo workspace. Test separately with `just ui-build`
  and `just ui-test`
- Bandwidth counted at the manager's unified write path using
  `zone_colors.total_bytes()`. Uniform across all 11 driver families, but payload-only
  (no transport headers). Field is named `payload_bps_estimate` to reflect this
- Rate computation lives in one place: a 2Hz collector task that writes a snapshot to
  `ArcSwap<DeviceMetricsSnapshot>` in `AppState`. REST handler reads the swap — all
  callers see identical data for the same snapshot window
- `last_error` strings are capped (length) and sanitized (strip newlines, cap size).
  Structural sanitization (stripping IPs, paths) deferred to a backend-by-backend pass
  in v2

---

## Out of Scope (v1)

- WebSocket per-device streaming (deferred to v2 with its own topic)
- Per-transport metrics: USB retries, network RTT, Hue entertainment stream health
- p95/p99 latency per device — only averages in v1
- Historical persistence beyond the UI's in-memory ring buffer
- Reconnect/state-transition counters
- Dashboard/fleet-view aggregations
- Wire-byte-accurate bandwidth (requires per-backend instrumentation)

---

## Implementation Plan

### Wave 1 — Daemon data layer

**Task 1.1 — Extend `DeviceOutputStatistics` with cumulative counters**

- Files: `crates/hypercolor-core/src/device/manager.rs`
- Add fields:
  - `bytes_sent: u64` — incremented by `zone_colors.total_bytes()` on write success
  - `errors_total: u64` — incremented on async write failure (complements the existing
    frame-level errors list)
- Update `debug_snapshot()` to include new fields
- Verify:
  - `just test-crate hypercolor-core`
  - Unit test: simulate N writes with known payload size, assert
    `bytes_sent == N × expected_payload`
  - Unit test: inject failures, assert `errors_total` matches

### Wave 2 — Daemon REST API

**Task 2.1 — `DeviceMetricsCollector` background task**

- Files: new `crates/hypercolor-daemon/src/device_metrics.rs`; `AppState` gains an
  `Arc<ArcSwap<DeviceMetricsSnapshot>>` field
- Collector runs at 2Hz (500ms interval, matching the current WS metrics default):
  1. Snapshot current `DeviceOutputStatistics` for every device
  2. Compute deltas vs previous snapshot + elapsed time → `fps_actual`,
     `payload_bps_estimate`
  3. Build `DeviceMetricsSnapshot { taken_at: Instant, devices: Vec<DeviceMetrics> }`
  4. `arc_swap.store(Arc::new(snapshot))`
- First tick after startup yields zero rates (no previous snapshot); subsequent ticks are
  delta-accurate. Reconnects are a non-issue because rates come from one source
- `DeviceMetrics` shape (lean):
  ```rust
  pub struct DeviceMetrics {
      pub id: DeviceId,
      pub fps_actual: f32,
      pub fps_target: u32,
      pub payload_bps_estimate: u64,
      pub avg_latency_ms: u32,
      pub frames_sent: u64,
      pub frames_dropped: u64,
      pub errors_total: u64,
      pub last_error: Option<String>,   // length-capped, newline-stripped
      pub last_sent_ago_ms: Option<u64>,
  }
  ```
- Verify:
  - `just test-crate hypercolor-daemon`
  - Unit test: advance mock clock, assert FPS/bandwidth derivation is correct
  - Unit test: two concurrent readers see identical values

**Task 2.2 — `GET /api/v1/devices/metrics` endpoint**

- Files: `crates/hypercolor-daemon/src/api/devices/mod.rs`
- Handler loads the `ArcSwap`, returns the snapshot wrapped in the standard envelope
- Response type: `{ data: Vec<DeviceMetrics>, meta: { ... taken_at_ms: i64 } }` — the
  timestamp lets clients detect stale responses
- Verify:
  - `curl localhost:9420/api/v1/devices/metrics | jq` returns expected shape
  - `crates/hypercolor-daemon/tests/api_tests.rs` — add REST shape test

### Wave 3 — UI polling + presentation

**Task 3.1 — Devices metrics context + fetcher**

- Files:
  - `crates/hypercolor-ui/src/api/devices.rs` — add `DeviceMetrics` struct and
    `fetch_device_metrics()`
  - New: `crates/hypercolor-ui/src/context/device_metrics.rs`
- Context provides a single `RwSignal<HashMap<DeviceId, DeviceMetricsState>>` where
  `DeviceMetricsState` holds the current snapshot plus a `VecDeque<f32>` ring buffer of
  last 60 FPS samples
- Provider polls `GET /api/v1/devices/metrics` at 2Hz, stopping when no consumer is
  mounted (driven by provider lifecycle, not WS subscription)
- On each poll:
  1. Diff the returned device IDs against current map keys
  2. Remove entries for devices that have disappeared (hotplug cleanup)
  3. For each returned device, push FPS into the ring buffer, update current values
- Verify:
  - `just ui-build`, `just ui-test`
  - Manual: unplug a device, confirm its card + buffer vanish within one poll cycle

**Task 3.2 — Device card metrics strip**

- Files:
  - New: `crates/hypercolor-ui/src/components/device_metrics_strip.rs`
  - `crates/hypercolor-ui/src/components/device_card.rs` mounts the strip on connected
    cards only
- Layout (single line at the bottom of the card):
  `{fps_actual}/{fps_target} fps · {payload_rate} · {errors}`
- Coloring via SilkCircuit tokens:
  - FPS value: Success Green if `fps_actual >= fps_target × 0.9`, Electric Yellow if
    within 10–30% below, Error Red if < 70% of target
  - Bandwidth: neutral (no hot/cold semantics)
  - Error count: Electric Yellow if `errors_total > 0`, Error Red if `last_sent_ago_ms`
    says the last error occurred within the last 5 seconds
- Hover on error count shows `last_error` tooltip
- FPS sparkline (`perf_charts::Sparkline`) rendered inline at right edge of the strip —
  60 samples in Neon Cyan
- Verify:
  - `just ui-dev` + visual check against running daemon
  - Toggle a device offline, confirm readout reflects error state
  - Play an effect, confirm the FPS line rises
  - Unplug the device, confirm the strip vanishes (not just goes blank)

### Wave 4 — Tests and docs

**Task 4.1 — API tests + spec doc**

- Files:
  - `crates/hypercolor-daemon/tests/api_tests.rs`
  - New: `docs/specs/XX-device-metrics.md` (number resolved at write time; short spec
    covering endpoint shape, UI expectations, sanitization rules, v2 open items)
- Verify: `just verify` passes clean

---

## Execution Order and Parallelism

| Wave | Tasks | Parallel? | Depends on |
|------|-------|-----------|------------|
| 1    | 1.1   | —         | —          |
| 2    | 2.1, 2.2 | 2.2 depends on 2.1 | Wave 1 |
| 3    | 3.1, 3.2 | 3.2 depends on 3.1 | Wave 2 |
| 4    | 4.1   | —         | Waves 2, 3 |

Waves 1–2 ship as a **daemon-only PR** (no UI change, REST endpoint live with no
consumers). Waves 3–4 as a **second UI PR**. Estimate: 3–5 hours focused work.

---

## Open Questions (resolved vs deferred)

1. **Bandwidth naming.** Resolved: `payload_bps_estimate`, documented as payload-only.
2. **Throttling and WS.** Resolved: no WS in v1. REST + polling at 2Hz.
3. **Delta computation location.** Resolved: centralized collector → `ArcSwap`.
4. **p95 latency in v1?** Deferred to v2 — average only ships now.
5. **Sparkline: FPS only or FPS + bandwidth?** Resolved: FPS sparkline only; bandwidth
   is a number. Revisit after real-device visual review.
6. **Last-error sanitization depth.** v1 caps length + strips newlines. Structural
   sanitization (IPs, paths) deferred to v2 — needs per-backend audit.
7. **Spec doc numbering.** Resolve next unused number at write time.

---

## v2 Roadmap (explicitly deferred)

- Dedicated WS topic `device_metrics` with its own subscription config, so the dashboard
  doesn't pay the cost
- p95/p99 latency histograms per device
- Transport-specific telemetry: USB retries, network RTT probes, WLED/DDP packet loss,
  Hue entertainment stream health
- Wire-byte-accurate bandwidth (per-backend instrumentation)
- Structural `last_error` sanitization (strip IPs, paths, tokens)
- Dashboard fleet view

---

## Appendix: Reference Links

- `DeviceOutputStatistics` definition:
  `crates/hypercolor-core/src/device/manager.rs:316-340`
- Aggregate WS metrics payload:
  `crates/hypercolor-daemon/src/api/ws/protocol.rs:691-704`
- WS metrics relay (per-client, timer-driven):
  `crates/hypercolor-daemon/src/api/ws/relays.rs:901-964`
- WS metrics interval validation (100-10000ms):
  `crates/hypercolor-daemon/src/api/ws/protocol.rs:269-275`
- UI default metrics subscription (500ms, all pages):
  `crates/hypercolor-ui/src/ws/connection.rs:170-176`
- Existing devices REST handler:
  `crates/hypercolor-daemon/src/api/devices/mod.rs:154-247`
- Devices page: `crates/hypercolor-ui/src/pages/devices.rs:78`
- Device card: `crates/hypercolor-ui/src/components/device_card.rs`
- Sparkline primitive: `crates/hypercolor-ui/src/components/perf_charts.rs:13-128`
